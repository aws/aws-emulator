// SPDX-License-Identifier: CC0-1.0

//! How a service states the framework's boundary refusal of a request.
//!
//! The framework hands a service the refusal STRUCTURALLY — `Refusal::Invalid` holding
//! one [`Violation`] per constraint the request broke — and lets the service choose the
//! words, because no model records them. [`aws_validation_refusal`] is the wording AWS
//! itself uses, so a service only has to delegate to it.
//!
//! One thing AWS words differently from the framework is how a `@pattern` requirement
//! is quoted, and that rendering is NOT implemented here: it is
//! [`crate::validation::unanchored`], this crate's staged home for it, which every service
//! carrying its own inlined copy is to migrate onto. This module only decides WHICH
//! violations get restated and in what envelope. The envelope itself, the member paths and
//! every other requirement are the framework's, so nothing about them can drift per crate.
//!
//! A service whose recordings show the other of the two renderings that home offers
//! ([`crate::validation::unanchored_grouping_alternation`], which keeps a top-level
//! alternation grouped) delegates to [`aws_validation_refusal_with`] and passes it: the
//! rendering is the only thing a service chooses here.
//!
//! Not every service answers that envelope at all. One whose model declares no
//! `ValidationException`, and whose recordings show an error OF ITS OWN for a value one of
//! its `@pattern`s does not admit, states that one: [`parameter_pattern_refusal`] over the
//! sentence [`parameter_pattern_mismatch`], naming the parameter and the pattern its value
//! had to match. There too the service chooses only its error code, which of its patterns it
//! states that way, and the rendering of the expression — the sentence stays here.
//!
//! A service that refuses a parameter value that way for EVERY constraint class, and not for
//! `@pattern` alone, states the same sentence over the requirement the value broke:
//! [`parameter_value_mismatch`] (or [`parameters_value_mismatch`], for a requirement that
//! holds over several members together) over one of the requirement clauses
//! [`must_match_pattern`], [`pattern_requirement_unstated`], [`must_be_at_most`],
//! [`must_be_at_least`] and [`must_be_one_of`], with the parameter named as the API spells it
//! ([`api_parameter_path`]) rather than as the framework's walk does. Which clause a class
//! takes, and which refusals are stated this way at all, is the service's; the sentences and
//! the spelling stay here.
//!
//! The rendering rule is framework-level — it belongs in
//! `vendor/smithy-schema-server/src/constraints.rs`'s `Violation::requirement`, which a fix
//! may not change from here; once it learns this, delete this module and every
//! `override_refusal` that delegates to it.
//!
//! One refusal does NOT come from that envelope at all: a member modelled as a MAP is
//! validated against the operation's input as a JSON OBJECT, so a map over its `@length`
//! maximum is a schema finding about the object's property count rather than a clause
//! about a member's length. [`property_count_refusal`] states that one, for a service
//! whose recordings show it, and is asked before the envelope.
//!
//! [`too_many_properties`], [`missing_required_properties`], [`instance_type_mismatch`],
//! [`array_too_short`], [`pattern_mismatch`],
//! [`string_too_long`], [`string_too_short`], [`numeric_above_maximum`],
//! [`numeric_below_minimum`] and [`enum_value_not_found`] are the SENTENCES that layer states
//! for the constraint classes it reports on a member it read from the request BODY. A service whose front door words
//! those findings itself — because it has to tell a body member from one bound to the URI,
//! which the refusal seam cannot — builds them from here, so the sentences do not drift
//! per crate. [`expected_vendor`] is the one sentence here that is not a schema finding:
//! how the resource-tagging layer refuses an ARN of another vendor, which it decides
//! before any lookup.
//!
//! A service that states those findings for the `@pattern` class ALONE — every other
//! constraint class keeping the framework's wording, because no recording of that service
//! shows its words for them — has the whole message in one call:
//! [`pattern_constraint_report`], the bracketed list over [`pattern_mismatch`], which
//! declines any refusal holding another class so no message is half in one voice and half
//! in the other.
//!
//! The other difference this module owns is the VALUE half of a clause: a COLLECTION
//! (list or map) whose `@length` was broken is stated with no value at all —
//! `Value at '<member path>' failed to satisfy constraint: …` — where a scalar states the
//! value it submitted. There is no scalar rendering of a list or a map, so none is
//! invented.
//!
//! The same half is dropped for a member whose value the refusal WITHHOLDS: a member
//! modelled `@sensitive` is one the service never discloses, so its clause names the
//! member and the requirement and states no value — neither the submitted bytes nor the
//! `null` the framework writes for an absent one. WHICH members those are is a property of
//! each service's model, so the caller decides it and passes the decision to
//! [`aws_validation_refusal_withholding`]; the clause, the envelope and every other
//! requirement stay this module's and the framework's.
//!
//! A service whose recordings drop that half for a whole constraint CLASS instead — every
//! `@pattern` requirement, say, whatever member broke it, because the requirement is the
//! expression and not the one value that missed it — cannot state that as a set of member
//! paths, since the same member states its value for one requirement and withholds it for
//! another. It passes the decision to [`aws_validation_refusal_withholding_violations`],
//! which asks it about the VIOLATION; that is the only difference between the two, and both
//! run the one body below.
//!
//! One rendering here is not a sentence but an ORDER: [`in_sorted_enum_order`] states an
//! enumerated type's value set lexicographically, which is how AWS lists it where a model
//! declares its members in an order of its own. A service hands its violations through it
//! before the envelope is rendered, so the clause itself stays the framework's. A service
//! whose recordings show an order that is neither the model's nor lexicographic — its own
//! published set, which may also list FEWER values than the model declares — states that
//! one instead: [`in_stated_enum_order`] over the rows it publishes
//! ([`StatedEnumValueSets`]), recognised by the value SET so every member targeting one
//! enum is stated alike, with the rows' DATA living in that service's own crate. A
//! hand-authored operation reading its own request reaches the same rows, because the
//! shared request reader hands its refusal back structurally
//! ([`crate::event_stream::read_request`]). A third order is neither the model's,
//! lexicographic nor published but computed from the values themselves:
//! [`in_hashed_enum_order`], for a front door that holds the admissible values in a hash
//! table keyed on each value's text and enumerates the table rather than the declaration.
//! The other
//! order is over the CLAUSES rather than inside one: [`in_pattern_first_order`] states a
//! member's `@pattern` requirement before that member's other requirements, for a front
//! door whose recordings show that order, with the members themselves left where the walk
//! found them.
//!
//! The same seam carries one more rendering of the VALUE half: a floating-point value is
//! quoted back the way a service formats one ([`float_value_text`], applied to a violation
//! by [`in_float_value_rendering`]) rather than as Rust's decimal expansion of it, which
//! prints a whole `5.0` as `5` and a small `-1.0E-6` as `-0.000001`. WHICH members are
//! floating point is a property of each service's model, which the runtime schemas carry
//! as the member's shape type, so the service passes that decision in.
//!
//! One constraint class reaches no refusal seam at all: a map's KEY, which the walk treats
//! as a path segment rather than as a constrained leaf, so nothing is reported about it and
//! the HANDLER applies the key shape's own constraints. [`map_key_refusal`] is the message
//! it states — the framework's envelope and clause, over the map member, listing what a key
//! has to satisfy — so a request refused for its key reads like one refused for its value.
//! [`map_key_refusal_quoting`] is the same message for a service whose recordings QUOTE the
//! map that key was held in, and for a key shape that declares no `@pattern`.
//!
//! The last difference this module owns is WHERE a constraint broken by a collection's
//! ELEMENTS is stated. The framework reports such a violation at the offending element —
//! `<collection>.<1-based position>.member`, echoing that element and naming only the
//! constraint it broke — while AWS states it once at the COLLECTION MEMBER, with the
//! element shape's WHOLE declared constraint set nested inside
//! `Member must satisfy constraint: [ … ]` and no value at all.
//! [`aws_validation_refusal_over_elements`] is that rendering, for a service whose
//! recordings show it; the set is read off the MODEL (the schemas the caller hands over),
//! because the refusal carries only the constraints that failed. A service whose OTHER
//! clauses it cannot state — one withholding the value a present member submitted while
//! still reporting an absent member as the `null` it was, which no per-member-path decision
//! expresses — reads that same set through [`element_constraint_set`] and nests it with
//! [`element_set_requirement`], so the wrapper and the set stay here either way. A service
//! whose recordings show a set this module's reading does not produce — one stating that an
//! element of a list that is not `@sparse` also has to be present, or one whose list members
//! are nested inside structures, which are looked up under their own member names — states
//! that set itself and hands it to [`aws_validation_refusal_over_elements_stating`], which
//! keeps every other clause, the value half and the envelope here.

use std::collections::{BTreeSet, HashMap, HashSet};

use aws_smithy_schema::{Schema, ShapeId, ShapeType, TraitMap};
use aws_smithy_schema_server::constraints::{
    self, ElementConstraintsTrait, EnumTrait, LengthTrait, PatternTrait, RangeTrait,
};
use aws_smithy_schema_server::{envelope, Violation, ViolationKind};
use smithy_server_runtime::{Refusal, Refused};

use crate::validation::{list_of_element, unanchored};

/// The two suffixes the validating walk renders a COLLECTION's size with when it
/// length-checks one: a list as `<n> items`, a map as `<n> entries`
/// (`aws_smithy_schema_server::constraints`'s collection-length check).
const COLLECTION_SIZE_SUFFIXES: [&str; 2] = [" items", " entries"];

/// The size `stored` states when it is the walk's rendering of a collection ending in
/// `suffix`, and `None` for anything else — so a clause whose echo is a caller's own
/// string is never read as a size.
///
/// The rendering is the ONLY signal available here: [`ViolationKind`] carries the
/// constraint that was broken and not the KIND of shape that broke it
/// (`vendor/smithy-schema-server/src/constraints.rs`), and the module header above states
/// why that crate cannot be changed from here. The accepted collision is a STRING member
/// whose submitted value is literally `<digits> items`/`<digits> entries` and whose
/// `@length` bound that string breaks: such a refusal is stated as a collection's. No
/// emulated member admits that today (the shortest `@length` maximum is far above the
/// seven characters the shortest such string takes), and the alternative — reading the
/// shape kind — needs the frozen crate.
fn collection_size(stored: &str, suffix: &str) -> Option<usize> {
    stored.strip_suffix(suffix)?.parse::<usize>().ok()
}

/// How many entries the framework's rendering of a MAP member states, when `stored` is
/// one. `None` for a list's rendering and for a caller's own string.
///
/// `pub` because a service whose front door words its OWN schema finding (rather than
/// delegating the whole refusal to [`property_count_refusal`]) needs the same reading of
/// the same rendering: two services cannot disagree about how many entries one map held.
#[must_use]
pub fn map_entry_count(stored: &str) -> Option<usize> {
    collection_size(stored, " entries")
}

/// How many items the framework's rendering of a LIST member states, when `stored` is one.
/// `None` for a map's rendering and for a caller's own string.
///
/// The mirror of [`map_entry_count`], and `pub` for the same reason: a service that words a
/// list's own `@length` refusal itself — because the sentence its recordings show names the
/// cap the SERVICE enforces rather than the clause the framework renders — reads the size out
/// of the same rendering, so no two services disagree about how many items one list held.
#[must_use]
pub fn list_item_count(stored: &str) -> Option<usize> {
    collection_size(stored, " items")
}

/// The clause a COLLECTION member's `@length` violation states — the framework's own,
/// with the `Value '<…>'` half reduced to `Value` — or `None` when `violation` is not one.
///
/// A list or a map has no scalar rendering, so the clause names the member and the
/// requirement and nothing else; the requirement half, the path and the envelope stay the
/// framework's, so only the value half can differ.
fn collection_length_clause(violation: &Violation) -> Option<String> {
    if !matches!(
        violation.kind,
        ViolationKind::MinLength(_) | ViolationKind::MaxLength(_)
    ) {
        return None;
    }
    let stored = violation.value.as_deref()?;
    COLLECTION_SIZE_SUFFIXES
        .iter()
        .find_map(|suffix| collection_size(stored, suffix))?;
    valueless_clause(violation)
}

/// `violation` with the value half of its clause replaced by the COLLECTION the caller
/// submitted, where the framework's walk stored its own summary of one.
///
/// The other direction from [`collection_length_clause`], for a service whose recordings
/// QUOTE the collection rather than dropping the value half: the walk renders a list as
/// `<n> items` and a map as `<n> entries`, neither of which any caller sent, while the echo
/// ([`crate::validation::SubmittedEcho`], collected by the walk that owns every bound on
/// what one clause may quote) is the collection itself.
///
/// ONLY that one clause is touched: a violation whose stored value is already the caller's
/// own bytes keeps it, and a collection the walk could not render — one past the bounds that
/// walk states, or one whose elements have no text rendering — keeps the summary rather than
/// losing the clause altogether.
///
/// THE HOME for this restoring rule, beside the two readings of the summary it rests on:
/// `services/service-com-amazonaws-eventbridge-awsevents/src/service.rs` carries its own
/// inlined copy, written before this module had one, and is to be MIGRATED onto this entry
/// point rather than copied again.
#[must_use]
pub fn quoting_the_submitted_collection(
    violation: Violation,
    echo: crate::validation::SubmittedEcho,
) -> Violation {
    let summarised = violation.value.as_deref().is_some_and(|stored| {
        matches!(
            violation.kind,
            ViolationKind::MinLength(_) | ViolationKind::MaxLength(_)
        ) && COLLECTION_SIZE_SUFFIXES
            .iter()
            .any(|suffix| collection_size(stored, suffix).is_some())
    });
    match echo {
        crate::validation::SubmittedEcho::Rendered(value) if summarised => Violation {
            value: Some(value),
            // The caller asked for the submitted collection to be QUOTED, so the clause
            // states it: a value restored on purpose is not one the walk withheld.
            sensitive: false,
            ..violation
        },
        _ => violation,
    }
}

/// `violation` with the clause's value half restored to the WHOLE value the caller
/// submitted, where the framework stored its own elision of it.
///
/// The framework's constraint walk elides the rejected value into the clause at a ceiling of
/// its own — a byte-bounded prefix plus `... [<n> bytes]`
/// (`vendor/smithy-schema-server/src/constraints.rs`) — which is wire text AWS never emits:
/// an AWS constraint clause quotes the value the request carried, however long that value
/// was. The echo ([`crate::validation::SubmittedEcho`], collected by the walk that owns every
/// bound on what one clause may quote) is that value, so the clause is restored from it.
///
/// ONLY that one clause is touched: the stored echo has to BE the framework's rendering of
/// the submitted value — identical, or its prefix plus the marker naming its byte count — so
/// a clause the framework wrote from something else (a collection's size summary, which
/// [`quoting_the_submitted_collection`] answers) keeps the framework's own text, and so does
/// a value the echo bounds declined to render. A violation whose value the boundary walk
/// WITHHELD is returned untouched, whatever the echo holds: what that clause states is the
/// constraint and no value, and no caller of this entry point can opt out of it.
#[must_use]
pub fn quoting_the_whole_submitted_value(
    violation: Violation,
    echo: &crate::validation::SubmittedEcho,
) -> Violation {
    let crate::validation::SubmittedEcho::Rendered(submitted) = echo else {
        return violation;
    };
    // A value the validating walk WITHHELD (a `@sensitive` member's, dropped as the
    // violation was recorded) leaves the clause with no value half to restore FROM, so it
    // is restored to exactly what that walk would have stored had it not withheld it: the
    // submitted value under the framework's own echo ceiling. Only the redaction is
    // undone, never the framework's elision — a service whose clause quotes the whole
    // value states its own ceiling for that.
    if violation.sensitive {
        return Violation {
            value: Some(
                crate::validation::quoted_echo_up_to(
                    submitted,
                    crate::validation::FRAMEWORK_ECHO_CEILING,
                )
                .into_owned(),
            ),
            sensitive: false,
            ..violation
        };
    }
    let elided = violation.value.as_deref().is_some_and(|stored| {
        stored != submitted && crate::validation::is_elision_of(stored, submitted)
    });
    if elided {
        return Violation {
            value: Some(submitted.clone()),
            // The caller asked for the submitted value to be QUOTED whole, so the clause
            // states it: a value restored on purpose is not one the walk withheld.
            sensitive: false,
            ..violation
        };
    }
    violation
}

/// The clause `violation` states with its VALUE half dropped — `Value at '<member path>'
/// failed to satisfy constraint: <requirement>` — whichever half the framework wrote: the
/// submitted value a present member echoes, or the `null` an ABSENT member is stated with.
///
/// Built by removing that half from the framework's own clause rather than by spelling a
/// clause here, so the requirement, the member path and the punctuation around them stay
/// single-homed in the framework and cannot drift. `None` when the framework's clause does
/// not start with the value half this would remove.
///
/// `pub` for the same reason [`envelope_over`] is: a service that assembles the clauses of
/// its own report — because it GROUPS some of them, which no entry point here can do for it
/// — still drops the value half of a clause that states none, and a second spelling of that
/// drop per crate is how one service comes to word it differently from another.
#[must_use]
pub fn valueless_clause(violation: &Violation) -> Option<String> {
    let value_half = match violation.value.as_deref() {
        Some(stored) => format!("Value '{stored}' "),
        // How the framework writes a member that was not there at all.
        None => "Value null ".to_string(),
    };
    let requirement_half = violation.message().strip_prefix(&value_half)?.to_string();
    Some(format!("Value {requirement_half}"))
}

/// The sentence a JSON-schema layer states for an object carrying more properties than
/// the schema admits — the count the request submitted and the maximum the model
/// declares, and nothing else.
#[must_use]
pub fn too_many_properties(found: usize, max: i64) -> String {
    format!("object has too many properties (found {found} but schema requires at most {max})")
}

/// The finding that layer states for a string outside the regular expression its schema
/// declares: the expression as the MODEL writes it (anchors included — this is the schema's
/// own expression, not the per-member clause's rendering that [`unanchored`] owns) and the
/// string the caller submitted.
///
/// `services/service-com-amazonaws-account-account/src/service.rs` and
/// `services/service-com-amazonaws-oam-oamservice/src/service.rs` still carry their own
/// inlined copies of this sentence, written before this module had it; they are to be
/// migrated onto this one rather than copied again.
#[must_use]
pub fn pattern_mismatch(pattern: &str, value: &str) -> String {
    format!("ECMA 262 regex \"{pattern}\" does not match input string \"{value}\"")
}

/// The finding that layer states for a string longer than the schema's maximum: the string
/// the caller submitted, its length, and that maximum.
///
/// `length` is the length the CONSTRAINT counted (`@length` counts characters), passed in
/// rather than measured here, so the number the finding states is the one that was checked.
/// The same two services as [`pattern_mismatch`] carry their own copies of this sentence.
#[must_use]
pub fn string_too_long(value: &str, length: usize, max: i64) -> String {
    format!("string \"{value}\" is too long (length: {length}, maximum allowed: {max})")
}

/// The finding that layer states for a string shorter than the schema's minimum: the
/// string the caller submitted, its length, and that minimum.
///
/// The mirror of [`string_too_long`], down to where the number comes from: `length` is
/// the length the CONSTRAINT counted, passed in rather than measured here.
#[must_use]
pub fn string_too_short(value: &str, length: usize, min: i64) -> String {
    format!("string \"{value}\" is too short (length: {length}, required minimum: {min})")
}

/// The finding that layer states for an OBJECT that does not carry every property its
/// schema requires: the properties the request left out, each double-quoted and separated
/// by a bare comma inside one pair of brackets.
///
/// Each property is named as the SCHEMA declares it — the name the property has on the
/// wire, not the member path a per-member clause states — so the caller reads back the
/// spelling it would have had to send. The caller hands the names over in the order the
/// finding is to list them.
#[must_use]
pub fn missing_required_properties<S: AsRef<str>>(properties: &[S]) -> String {
    let quoted: Vec<String> = properties
        .iter()
        .map(|property| format!("\"{}\"", property.as_ref()))
        .collect();
    format!(
        "object has missing required properties ([{}])",
        quoted.join(",")
    )
}

/// The finding that layer states for an instance that is not of a type its schema admits:
/// the type the instance HAS, then every type the schema allows, each double-quoted and
/// separated by a bare comma inside one pair of brackets.
///
/// `found` and `allowed` are spelled in JSON's own type vocabulary — `null`, `boolean`,
/// `integer`, `number`, `string`, `array`, `object` — which is what a schema declares a type
/// in and therefore what the finding reports. A service that validates a DOCUMENT it was
/// handed (rather than a modelled member) states the same sentence about that document's
/// own schema.
#[must_use]
pub fn instance_type_mismatch<S: AsRef<str>>(found: &str, allowed: &[S]) -> String {
    let quoted: Vec<String> = allowed
        .iter()
        .map(|admitted| format!("\"{}\"", admitted.as_ref()))
        .collect();
    format!(
        "instance type ({found}) does not match any allowed primitive type (allowed: [{}])",
        quoted.join(",")
    )
}

/// The finding that layer states for an ARRAY carrying fewer elements than its schema
/// admits: the minimum the schema declares and the count the instance carries.
///
/// The mirror of [`string_too_short`] for the other shape a length minimum bounds: an array
/// has no scalar rendering to quote, so the finding states the two counts and no value.
#[must_use]
pub fn array_too_short(found: usize, min: i64) -> String {
    format!(
        "array is too short: must have at least {min} elements but instance has {found} elements"
    )
}

/// The finding that layer states for a number above the maximum its schema admits: that
/// maximum, then the number the caller submitted.
///
/// Both are passed as the TEXT they are stated with — the bound as the schema writes it and
/// the value as the request carried it — so neither is re-rendered here and an integral
/// bound never grows a fractional part it was not declared with.
#[must_use]
pub fn numeric_above_maximum(value: &str, max: &str) -> String {
    format!(
        "numeric instance is greater than the required maximum (maximum: {max}, found: {value})"
    )
}

/// The finding that layer states for a number below the minimum its schema admits: the
/// mirror of [`numeric_above_maximum`], down to where its two numbers come from.
#[must_use]
pub fn numeric_below_minimum(value: &str, min: &str) -> String {
    format!("numeric instance is lower than the required minimum (minimum: {min}, found: {value})")
}

/// The finding that layer states for a value outside the set its schema enumerates: the
/// value the caller submitted, then every value the schema admits, each quoted and
/// separated by a bare comma.
///
/// `permitted` is stated in the order it is handed over — the order the MODEL declares the
/// enumerated type's members in, which is the order the validating walk carries — so the
/// finding lists the type as its definition reads. An emulated service that validates a
/// DOCUMENT it was handed (rather than a modelled member) states the same sentence about
/// the values that document's format defines;
/// `services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921/src/state.rs`
/// carries its own copy for that layer, written before this module had the sentence, and
/// is to be migrated onto this one rather than copied again.
#[must_use]
pub fn enum_value_not_found<S: AsRef<str>>(value: &str, permitted: &[S]) -> String {
    let quoted: Vec<String> = permitted
        .iter()
        .map(|admitted| format!("\"{}\"", admitted.as_ref()))
        .collect();
    format!(
        "instance value (\"{value}\") not found in enum (possible values: [{}])",
        quoted.join(",")
    )
}

/// `violation` with an enumerated type's value set stated in LEXICOGRAPHIC order, and any
/// other violation returned unchanged.
///
/// A refusal of a value outside an enumerated type names the values that type admits, and
/// AWS states that set sorted where a model declares its members in an order of its own
/// (release order, or simply the order the model was written in). The CLAUSE stays the
/// framework's: this only re-orders what goes into it, so the member path, the echoed value
/// and the requirement's wording keep coming from the one place that writes them.
///
/// The order is over the declared values themselves, a total order on strings, so one enum
/// is stated the same way whatever order a model revision lists it in — and a service wires
/// this in for EVERY enum of its model rather than for a table of them.
#[must_use]
pub fn in_sorted_enum_order(violation: &Violation) -> Violation {
    let ViolationKind::EnumValue(declared) = &violation.kind else {
        return violation.clone();
    };
    let mut sorted = declared.clone();
    sorted.sort_unstable();
    Violation {
        path: violation.path.clone(),
        kind: ViolationKind::EnumValue(sorted),
        value: violation.value.clone(),
        sensitive: violation.sensitive,
    }
}

/// `violation` with an enumerated type's value set stated in the order the set's own HASH
/// TABLE is walked, and any other violation returned unchanged.
///
/// The third of the three orders a refusal of a value outside an enumerated type is seen
/// listing that type's values in, beside [`in_sorted_enum_order`] (lexicographic) and
/// [`in_stated_enum_order`] (a set the service publishes): a front door that holds the
/// admissible values in a hash table keyed on each value's own text, and enumerates the
/// table rather than the type's declaration, lists them
///
/// * in ascending bucket index, the bucket being the low bits
///   ([`ENUM_VALUE_SET_BUCKETS`]) of the value's string hash — the 31-polynomial over its
///   UTF-16 code units ([`enum_value_bucket`]);
/// * a bucket holding more than one value (two values whose hashes agree in those low
///   bits) read from the head each value was pushed onto, so the LATER of the two is
///   stated first. That direction is shown by a recorded refusal of a set that HAS a
///   collision — `com.amazonaws.acm-pca#ValidityPeriodType`, declared `END_DATE`,
///   `ABSOLUTE`, `DAYS`, `MONTHS`, `YEARS`, is stated `END_DATE, MONTHS, YEARS, DAYS,
///   ABSOLUTE`: `ABSOLUTE` and `DAYS` share a bucket and the later-declared `DAYS` comes
///   first.
///
/// The width and the chain direction are what the recorded listings show, and those sets
/// are small — the two cited above hold six and five values. A type with MORE values than
/// the table has buckets would make the width observable in a way no recording here pins,
/// so a listing of one that does not match is a reason to revisit [`ENUM_VALUE_SET_BUCKETS`]
/// rather than to add a row somewhere.
///
/// The order is computed from the declared values themselves, so it belongs to the
/// enumerated TYPE and not to a member, an operation or a depth: one type is stated alike
/// wherever it is carried, and a type the model gains needs no row anywhere. It is the
/// type's up to the chain order of values that COLLIDE, which follows the order they were
/// declared in, so a model revision that reorders a type's declaration moves the stated
/// set only where two of its values share a bucket. The CLAUSE stays the framework's: this
/// only re-orders what goes into it, so the member path, the echoed value and the
/// requirement's wording keep coming from the one place that writes them. A service wires
/// this in for EVERY enum of its model rather than for a table of them.
///
/// THE HOME for this order: `service-com-amazonaws-acmpca-acmprivateca`,
/// `service-com-amazonaws-databrew-awsgluedatabrew` and
/// `service-com-amazonaws-partnercentralaccount-partnercentralaccount` each carry the same
/// derivation privately, written before this module had one, and are to be migrated onto
/// this function rather than copied again.
#[must_use]
pub fn in_hashed_enum_order(violation: &Violation) -> Violation {
    let ViolationKind::EnumValue(declared) = &violation.kind else {
        return violation.clone();
    };
    Violation {
        path: violation.path.clone(),
        kind: ViolationKind::EnumValue(
            hashed_enum_order(declared)
                .into_iter()
                .map(str::to_string)
                .collect(),
        ),
        value: violation.value.clone(),
        sensitive: violation.sensitive,
    }
}

/// How wide the table [`in_hashed_enum_order`] walks is, and so the modulus a value's
/// bucket is taken at.
pub const ENUM_VALUE_SET_BUCKETS: u32 = 32;

/// `declared` — an enumerated type's admissible values, in the order they reached the
/// clause — in the order [`in_hashed_enum_order`] states them, for a caller holding the
/// values rather than a [`Violation`].
#[must_use]
pub fn hashed_enum_order<S: AsRef<str>>(declared: &[S]) -> Vec<&str> {
    // The given order reversed FIRST, so the stable sort below leaves two values that
    // share a bucket in that reverse order: the chain is read from the head each value was
    // pushed onto.
    let mut stated: Vec<&str> = declared.iter().rev().map(AsRef::as_ref).collect();
    stated.sort_by_key(|value| enum_value_bucket(value));
    stated
}

/// Which bucket of the table `value` sits in: its string hash ([`string_hash`]) reduced to
/// the table's width.
///
/// The hash is a fold over values a MODEL declares; it reads nothing a caller sent and is
/// not a digest, so it decides a listing order and nothing else.
#[must_use]
pub fn enum_value_bucket(value: &str) -> u32 {
    string_hash(value) % ENUM_VALUE_SET_BUCKETS
}

/// The 31-based hash of one string: the hash so far multiplied by 31 plus the next UTF-16
/// code unit, wrapping at 32 bits.
///
/// The hash the layer in front of these services keys a string by, and the ONE derivation
/// of it here — the bucket an enumerated value is listed from ([`enum_value_bucket`]) and
/// the bucket a map's entry is stated from
/// ([`crate::submitted_collections::map_entry_bucket`]) are the same fold at two widths, so
/// the two orders cannot drift apart. It is not a digest and nothing is decided by it but a
/// listing ORDER.
#[must_use]
pub fn string_hash(value: &str) -> u32 {
    value.encode_utf16().fold(0u32, |hash, unit| {
        hash.wrapping_mul(31).wrapping_add(u32::from(unit))
    })
}

/// The violations of ONE refusal in the order a front door states them that reports a
/// member's `@pattern` requirement before that member's other requirements: the members
/// in the order the validating walk found them, and within one member the EXPRESSION it
/// must match first, because the expression is the shape of the value and every other
/// requirement is a bound on it.
///
/// Only the ORDER changes: each clause, its member path and its requirement stay the
/// framework's, so a service that takes this cannot word a constraint differently from one
/// that does not. A service hands its violations through this before the envelope is
/// rendered.
///
/// Each violation's key is computed ONCE, in a single pass that remembers where each member
/// was first reported, rather than inside the comparator: a comparator that rescanned the
/// slice would read each path once per pair it is asked about. The sort is STABLE, so two
/// violations this key cannot separate keep the order the walk collected them in, and the
/// walk hands over at most its own ceiling of violations
/// ([`crate::validation::MAX_VIOLATIONS`]), so the work is bounded by this crate rather
/// than by one request.
///
/// THE HOME for this order: `services/service-com-amazonaws-acmpca-acmprivateca`'s
/// `stated_order` carries the same single-pass loop, written before this module had one,
/// and is to be migrated onto this function rather than copied again.
#[must_use]
pub fn in_pattern_first_order(violations: &[Violation]) -> Vec<&Violation> {
    let mut first_reported_at: HashMap<&str, usize> = HashMap::with_capacity(violations.len());
    let mut keyed: Vec<((usize, bool), &Violation)> = violations
        .iter()
        .enumerate()
        .map(|(position, violation)| {
            let member = *first_reported_at
                .entry(violation.path.as_str())
                .or_insert(position);
            (
                (member, !matches!(violation.kind, ViolationKind::Pattern(_))),
                violation,
            )
        })
        .collect();
    keyed.sort_by_key(|(key, _)| *key);
    keyed.into_iter().map(|(_, violation)| violation).collect()
}

/// A service's own statement of the value sets it lists in an order of its own: one row
/// per enumerated type, holding the values as the MODEL declares them and the values as
/// the SERVICE states them, in the order it states them.
///
/// A row's two halves are matched as SETS, not pairwise, so the model half may be written
/// in any order — and the service half may hold FEWER values than the model declares,
/// which is what a service whose published set lags its schema lists. Which values a
/// member ADMITS is never decided here: that is the enum the member targets, so a value
/// missing from the service half is still accepted.
///
/// The values a row holds are the service's own data, so the rows live in the service
/// crate; everything done WITH them is below.
pub type StatedEnumValueSets<'s> = [(&'s [&'s str], &'s [&'s str])];

/// The values `stated` says a service lists for the enumerated type whose declared value
/// set is `declared`, and `None` for a set it pins no order of.
///
/// `declared` is matched as a SET: a refusal reports the values a member admits rather
/// than the shape they came from, so every member targeting one enum — in any operation,
/// in whatever binding carried the value, at any depth — is stated alike, and the order
/// the schema happened to present them in does not decide whether the row is found.
#[must_use]
pub fn stated_enum_value_set<'s>(
    declared: &[String],
    stated: &StatedEnumValueSets<'s>,
) -> Option<&'s [&'s str]> {
    let presented: BTreeSet<&str> = declared.iter().map(String::as_str).collect();
    stated
        .iter()
        .find(|(modelled, _)| {
            modelled.len() == presented.len()
                && modelled.iter().all(|value| presented.contains(value))
        })
        .map(|(_, service)| *service)
}

/// `violation` with an enumerated type's value set stated as the SERVICE lists that set
/// ([`stated_enum_value_set`]), and any other violation — and any set `stated` pins no
/// order of — returned unchanged.
///
/// The counterpart of [`in_sorted_enum_order`] for a service whose order is its own rather
/// than lexicographic. The CLAUSE stays the framework's: only what goes into it moves, so
/// the member path, the echoed value and the requirement's wording keep coming from the one
/// place that writes them. A service hands its violations through this before the envelope
/// is rendered, and reads back from the result whether anything moved at all.
#[must_use]
pub fn in_stated_enum_order(violation: &Violation, stated: &StatedEnumValueSets<'_>) -> Violation {
    let ViolationKind::EnumValue(declared) = &violation.kind else {
        return violation.clone();
    };
    let Some(service) = stated_enum_value_set(declared, stated) else {
        return violation.clone();
    };
    Violation {
        path: violation.path.clone(),
        kind: ViolationKind::EnumValue(
            service.iter().map(|value| (*value).to_string()).collect(),
        ),
        value: violation.value.clone(),
        sensitive: violation.sensitive,
    }
}

/// The magnitudes a floating-point value is quoted back in PLAIN decimal form: at least
/// a thousandth, and below ten million. A value outside that band is quoted in scientific
/// notation ([`float_value_text`]).
const PLAIN_DECIMAL_BAND: std::ops::Range<f64> = 1e-3..1e7;

/// The text a constraint-violation clause quotes a FLOATING-POINT value back with.
///
/// A service writes a floating-point value the way it formats one, which is NOT Rust's
/// `to_string`: the value always carries a fractional part or an exponent (a whole `5`
/// reads `5.0`, never `5`), and a magnitude outside [`PLAIN_DECIMAL_BAND`] is written in
/// scientific notation — one digit before the point, at least one after it, then `E` and
/// the signed exponent (`-0.000001` reads `-1.0E-6`, `12345678` reads `1.2345678E7`) —
/// rather than expanded into the decimal digits of the same value. The digits themselves
/// are the shortest that round-trip the value, so nothing is added to or dropped from
/// what was submitted. A value with no decimal form at all (an infinity, a NaN) keeps
/// Rust's own rendering rather than being given a fake one.
///
/// It shares its "a whole value grows a fractional part" step with
/// [`crate::query_decimal_floats::decimal_double_text`], but is NOT that rendering and
/// must not be folded into it: that one is the XML WIRE form of a member's value, which
/// AWS writes as a plain decimal at every magnitude, while a validation clause quotes the
/// value the way the service's own formatter prints it — scientific notation included.
#[must_use]
pub fn float_value_text(value: f64) -> String {
    if !value.is_finite() {
        return value.to_string();
    }
    if value == 0.0 || PLAIN_DECIMAL_BAND.contains(&value.abs()) {
        let plain = value.to_string();
        return if plain.contains('.') {
            plain
        } else {
            format!("{plain}.0")
        };
    }
    // `{:e}` is the shortest text that round-trips the value in exponent form
    // (`-1e-6`); the mantissa then grows the fractional digit this form always carries.
    let exponent_form = format!("{value:e}");
    let (mantissa, exponent) = exponent_form
        .split_once('e')
        .unwrap_or((exponent_form.as_str(), "0"));
    let mantissa = if mantissa.contains('.') {
        mantissa.to_string()
    } else {
        format!("{mantissa}.0")
    };
    format!("{mantissa}E{exponent}")
}

/// The validation paths of the members `shapes` types as `float` or `double`, built off the
/// MODEL rather than named by hand.
///
/// `shapes` are the shapes a violation path of the calling service can name a member of,
/// each with the path prefix the validating walk reports that shape's members under
/// (the empty string for an operation input's own members). Codegen emits a member schema
/// WITHOUT its target's own members (`Schema::new_member` takes a shape type and a name),
/// so a NESTED shape is passed as its own schema under the member path that binds it.
///
/// A service hands the result to [`in_float_value_rendering`], so the two halves of that
/// rule — which members are floating point, and how a floating-point value is written —
/// have ONE owner each. THE HOME for this collection: `codebuild-codebuild-20161006`
/// carries its own inlined copy of the same loop (`floating_point_member_paths`), written
/// before this module had one, and is to be migrated onto this function rather than copied
/// again — its migration is that service's own change, because its recordings also decide
/// which VALUE rendering it quotes back.
#[must_use]
pub fn floating_point_member_paths(shapes: &[(&str, &'static Schema)]) -> BTreeSet<String> {
    let mut paths = BTreeSet::new();
    for (prefix, shape) in shapes {
        for member in shape.members() {
            let Some(name) = member.member_name() else {
                continue;
            };
            if !matches!(member.shape_type(), ShapeType::Float | ShapeType::Double) {
                continue;
            }
            // The casing of a validation path segment belongs to the path, not to the
            // model, so it is spelled by the framework module that owns that spelling.
            let segment = constraints::path::member(name);
            paths.insert(if prefix.is_empty() {
                segment
            } else {
                format!("{prefix}.{segment}")
            });
        }
    }
    paths
}

/// `violation` with the value half quoted back the way the service writes a FLOATING-POINT
/// value ([`float_value_text`]), for a violation whose member `is_floating_point` answers
/// `true` for; every other violation is returned unchanged.
///
/// The framework renders every numeric leaf with Rust's `to_string`, which has no way to
/// know whether the member it is writing is an integral one — so WHICH members are
/// floating point is a property of each service's model and is passed in, by the member
/// path the clause names ([`floating_point_member_paths`] reads that set off the model).
/// The clause, the member path and the requirement stay the framework's: only the
/// submitted value's rendering changes, and only for a value that parses as a number.
#[must_use]
pub fn in_float_value_rendering(
    violation: &Violation,
    is_floating_point: impl Fn(&str) -> bool,
) -> Violation {
    let stated = violation.clone();
    let Some(shown) = violation.value.as_deref() else {
        return stated;
    };
    if !is_floating_point(&violation.path) {
        return stated;
    }
    let Ok(value) = shown.parse::<f64>() else {
        return stated;
    };
    Violation {
        value: Some(float_value_text(value)),
        ..stated
    }
}

/// The findings of one refusal as that layer LISTS them: `[<finding>, <finding>]`.
///
/// A JSON-schema layer reports what it found as a list, so one finding and five render
/// the same way. What a service puts in FRONT of the list is its own (the recordings do
/// not even agree on the punctuation before it), but the list itself is one rendering and
/// lives here.
#[must_use]
pub fn bracketed_findings(findings: &[String]) -> String {
    format!("[{}]", findings.join(", "))
}

/// The whole message a service states for a request whose values are outside the
/// `@pattern`s of the members they were sent for: that layer's bracketed list
/// ([`bracketed_findings`]) holding one [`pattern_mismatch`] finding per violated member,
/// in the order the validating walk found them.
///
/// The brackets are the list itself, so one violated member reads
/// `[ECMA 262 regex "…" does not match input string "…"]` and two state both findings
/// inside the one pair.
///
/// Both halves of a finding come from the request being refused: the expression from the
/// `@pattern` of the member the value was sent for, and the value from the request itself
/// (already elided by the framework's own echo ceiling before it reaches here). A member
/// the model marks `@sensitive` reaches here with its value withheld, so its finding
/// states the expression and no input string.
///
/// ALL OR NOTHING. `None` when the refusal carries no violation at all, or when any one of
/// them broke some other constraint than its member's `@pattern`: a message half in this
/// layer's voice and half in the framework's is worse than the framework's consistent
/// default, so such a refusal keeps the framework's whole wording. A service that states
/// this layer's findings for the OTHER constraint classes too builds its message from the
/// sentences beside [`pattern_mismatch`] over [`bracketed_findings`] instead.
#[must_use]
pub fn pattern_constraint_report(violations: &[Violation]) -> Option<String> {
    if violations.is_empty() {
        return None;
    }
    let findings: Vec<String> = violations
        .iter()
        .map(pattern_finding)
        .collect::<Option<Vec<_>>>()?;
    Some(bracketed_findings(&findings))
}

/// One finding of [`pattern_constraint_report`], or `None` when `violation` broke some
/// other constraint than its member's `@pattern`.
fn pattern_finding(violation: &Violation) -> Option<String> {
    let ViolationKind::Pattern(pattern) = &violation.kind else {
        return None;
    };
    Some(pattern_mismatch(
        pattern,
        violation.value.as_deref().unwrap_or_default(),
    ))
}

/// The message a service states for a MAP whose KEY broke one of the constraints the
/// map's KEY SHAPE declares — the one constraint class no walk ahead of a handler
/// reports, because that walk treats a map's key as a path segment rather than as a
/// constrained leaf, so the handler applies the key's constraints and words the refusal
/// with this.
///
/// It is the framework's own envelope and clause, over the map MEMBER: the key's
/// requirements are listed as what a key has to satisfy, each sentence the framework's
/// (so it cannot drift from the one a reported member gets), and the clause states no
/// value — a map has no scalar rendering to quote, which is the same half
/// [`collection_length_clause`] drops for a map's own `@length`.
///
/// `min_length` and `max_length` are the key shape's declared `@length` bounds and
/// `pattern` its declared `@pattern`, quoted as the caller hands it over: a service
/// whose refusals render an expression AWS's way passes it through
/// [`crate::validation::unanchored`] first, one that keeps the framework's rendering
/// passes the model's own.
#[must_use]
pub fn map_key_refusal(
    member_path: &str,
    min_length: i64,
    max_length: i64,
    pattern: &str,
) -> String {
    map_key_refusal_quoting(member_path, None, min_length, max_length, Some(pattern))
}

/// As [`map_key_refusal`], with the two halves of the clause a service's own recordings
/// decide: the MAP the offending key was held in, quoted as the value the clause is about
/// ([`crate::validation::rendered_map`] renders one), and the key shape's `@pattern` listed
/// only where that shape declares one.
///
/// `value` is `None` for the service whose recordings show the valueless clause — a map has
/// no scalar rendering, which is the same half [`collection_length_clause`] drops for a
/// map's own `@length` — and `Some` for the one whose recordings quote the map. `pattern` is
/// `None` for a key shape that declares none, so the bracketed list names what that shape
/// actually requires and nothing more.
#[must_use]
pub fn map_key_refusal_quoting(
    member_path: &str,
    value: Option<&str>,
    min_length: i64,
    max_length: i64,
    pattern: Option<&str>,
) -> String {
    let requirements: Vec<String> = [
        Some(requirement(ViolationKind::MaxLength(max_length))),
        Some(requirement(ViolationKind::MinLength(min_length))),
        pattern.map(|pattern| requirement(ViolationKind::Pattern(pattern.to_string()))),
    ]
    .into_iter()
    .flatten()
    .collect();
    map_key_requirements_refusal(member_path, value, &requirements)
}

/// As [`map_key_refusal_quoting`], with the requirements the key shape declares stated by
/// the CALLER rather than built from `@length` bounds and a `@pattern`.
///
/// The entry point for a key shape whose constraint the bounds-and-pattern parameters cannot
/// express — an enumerated TYPE, whose requirement is its value set
/// ([`requirement`] over [`ViolationKind::EnumValue`]) — and the one body
/// [`map_key_refusal_quoting`] itself runs, so the clause, the bracketed list and the
/// envelope stay single-homed however a service's key shape is constrained. Each
/// requirement is a sentence [`requirement`] wrote, so a requirement a handler lists cannot
/// be worded differently from the one a reported member gets, and the ORDER is the caller's,
/// because the order a set is stated in is part of what that service states.
///
/// `value` is the map as the clause quotes it
/// ([`crate::validation::rendered_map_in_key_order`] renders one) and `None` for the
/// valueless clause — the service whose recordings show no value, and the map whose
/// rendering is past the largest value a clause quotes.
#[must_use]
pub fn map_key_requirements_refusal(
    member_path: &str,
    value: Option<&str>,
    requirements: &[String],
) -> String {
    // The value half is the framework's own spelling of one, so a clause about a key reads
    // like a clause about a member the walk reported.
    let stated_value = match value {
        Some(value) => format!("Value '{value}' at"),
        None => "Value at".to_string(),
    };
    let clause = format!(
        "{stated_value} '{member_path}' failed to satisfy constraint: Map keys must satisfy \
         constraint: {}",
        bracketed_findings(requirements)
    );
    // The ENVELOPE is the framework's too: one placeholder violation about the same
    // member renders it, and that violation's own clause is then swapped for the one
    // about the key — the same substitution [`stated_refusal`] makes on a clause whose
    // value half it drops. `envelope` answers `None` only for no violation at all.
    let placeholder = Violation {
        path: member_path.to_string(),
        kind: ViolationKind::MissingRequiredMember,
        value: None,
        sensitive: false,
    };
    match envelope(std::slice::from_ref(&placeholder)) {
        Some(rendered) => rendered.replace(&placeholder.message(), &clause),
        None => clause,
    }
}

/// The REQUIREMENT half of a violation's clause — the `Member must …` sentence — taken
/// out of the framework's own clause rather than spelled here, so the requirements a
/// handler lists and the ones a reported member gets are the same sentences.
///
/// `pub` because a service that states its own clauses around a requirement — one whose
/// dialect this module's whole-refusal entry points do not cover — needs the same sentence
/// for the same constraint: the alternative is each such crate re-deriving it from
/// [`Violation::message`], which is how two crates come to word one constraint two ways.
#[must_use]
pub fn requirement(kind: ViolationKind) -> String {
    /// How the framework separates a clause's member half from its requirement.
    const CLAUSE_SEPARATOR: &str = "failed to satisfy constraint: ";

    let clause = Violation {
        path: String::new(),
        kind,
        value: None,
        sensitive: false,
    }
    .message();
    match clause.split_once(CLAUSE_SEPARATOR) {
        Some((_, requirement)) => requirement.to_string(),
        None => clause,
    }
}

/// The sentence the resource-tagging layer in front of a service refuses a caller-supplied
/// `resourceArn` of ANOTHER vendor with: it names the vendor it expected — the serving
/// service's own ARN namespace — and nothing of the ARN it was handed.
///
/// Not a schema finding like the three above: the layer refuses the ARN before it looks any
/// resource up (an ARN of another vendor can name none of this service's resources however
/// the account changes), so it states what it expected rather than what the value failed.
/// `services/service-com-amazonaws-emrserverless-awstoledowebservice/src/service.rs`
/// still carries its own inlined copy of this sentence; it is to be migrated onto this
/// one rather than copied again.
#[must_use]
pub fn expected_vendor(vendor: &str) -> String {
    format!("Invalid input resource arn: expected vendor {vendor}")
}

/// [`too_many_properties`] for one violation, or `None` when the violation is not a MAP
/// member over its `@length` maximum.
fn property_count_clause(violation: &Violation) -> Option<String> {
    let ViolationKind::MaxLength(max) = &violation.kind else {
        return None;
    };
    let found = map_entry_count(violation.value.as_deref()?)?;
    Some(too_many_properties(found, *max))
}

/// State `refusal` the way the JSON-SCHEMA layer in front of an AWS REST service states
/// an object with too many properties, or `None` when no member of the request broke a
/// map's `@length` maximum.
///
/// A member modelled as a MAP is validated against the operation's input as a JSON
/// OBJECT, so a map carrying more entries than the model's maximum is a schema failure
/// about the object's property count — reported as the bracketed LIST of schema
/// findings, `[<finding>, …]` — and not one of the per-member
/// `N validation error(s) detected: …` clauses the framework renders, which are what a
/// scalar member outside its own constraint produces. A service whose recordings show
/// that layer delegates its `Service::override_refusal` here FIRST and falls through to
/// the per-member envelope ([`aws_validation_refusal`]) when this answers `None`.
///
/// A request that breaks a map's maximum AND another member's constraint is stated as the
/// schema findings ALONE: the schema layer answers before the per-member walk is reported,
/// so its findings are the whole message rather than clauses mixed into the other
/// envelope.
///
/// The code travels with the message, as everywhere else in this module: `None` would
/// leave the framework's generic member-length clause, which is the text this exists to
/// replace.
pub fn property_count_refusal(refusal: &Refusal) -> Option<Refused> {
    let Refusal::Invalid(violations) = refusal else {
        return None;
    };
    let clauses: Vec<String> = violations.iter().filter_map(property_count_clause).collect();
    if clauses.is_empty() {
        return None;
    }
    Some(Refused {
        code: VALIDATION_EXCEPTION.to_owned(),
        message: bracketed_findings(&clauses),
    })
}

/// The error code AWS answers a constraint refusal with on the JSON protocols, and the
/// code the framework's own default picks there — supplied so a chosen message always
/// travels with its matching code.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// State `refusal` the way AWS states it, or `None` to keep the framework's own wording.
///
/// This is the whole `Service::override_refusal` body for a service on a JSON protocol
/// (restJson1 or the awsJson family), where AWS answers a constraint refusal as
/// `400 ValidationException` with the envelope
/// `N validation error(s) detected: Value '<value>' at '<member path>' failed to satisfy
/// constraint: <requirement>` — the envelope the framework already renders, except for two
/// renderings:
///
/// * AWS quotes a `@pattern` requirement without the anchors a whole-value pattern wraps
///   it in ([`crate::validation::unanchored`]). The check itself still uses the model's
///   pattern as written: only the RENDERING drops them.
/// * a COLLECTION (list or map) that broke its `@length` is stated with no value at all,
///   `Value at '<member path>' failed to satisfy constraint: <requirement>`, because a
///   list and a map have no scalar rendering to quote.
///
/// Every other requirement, and the member path, are the framework's, so the two cannot
/// drift.
///
/// `None` for anything that is not a constraint refusal (an unauthenticated caller keeps
/// the framework's 403 code and wording), and `None` when there is nothing to state.
///
/// A service on a query protocol must not wire this in unchanged: the framework answers
/// a boundary refusal there as `MalformedQueryString`, so forcing the JSON code would
/// change what a client branches on.
pub fn aws_validation_refusal(refusal: &Refusal) -> Option<Refused> {
    aws_validation_refusal_with(refusal, unanchored)
}

/// [`aws_validation_refusal`], with the members whose value the refusal WITHHOLDS chosen by
/// the caller: `withholds_value` is asked about a violation's member path, and a clause it
/// answers `true` for states no value at all.
///
/// A service passes the members its own model marks `@sensitive`, which the runtime schemas
/// do not carry. Everything else — which refusals are restated, the code, the envelope, the
/// member paths, the `@pattern` rendering and every other requirement — is the same body
/// [`aws_validation_refusal`] runs, so the two cannot drift.
pub fn aws_validation_refusal_withholding(
    refusal: &Refusal,
    withholds_value: impl Fn(&str) -> bool,
) -> Option<Refused> {
    stated_refusal(refusal, unanchored, |violation| {
        withholds_value(&violation.path)
    })
}

/// [`aws_validation_refusal_withholding`], with the clauses that state NO VALUE decided from
/// the VIOLATION rather than from its member path.
///
/// A service needs this when its recordings show the value withheld for a whole CONSTRAINT
/// CLASS — every `@pattern` requirement, say, whatever member broke it — which no set of
/// member paths can express: the same member states its value for one requirement and
/// withholds it for another, so the decision belongs to the violation and not to the path.
/// `withholds_value` is asked about each violation as it will be STATED (its `@pattern`
/// requirement already requoted), and everything else — which refusals are restated, the
/// code, the envelope, the member paths, the `@pattern` rendering, the clause count's bound
/// and every other requirement — is the same [`stated_refusal`] body every other entry point
/// here runs, so a class-keyed choice cannot drift into a second envelope.
pub fn aws_validation_refusal_withholding_violations(
    refusal: &Refusal,
    withholds_value: impl Fn(&Violation) -> bool,
) -> Option<Refused> {
    stated_refusal(refusal, unanchored, withholds_value)
}

/// [`aws_validation_refusal_withholding`], with the `@pattern` rendering chosen by the
/// caller too — the one entry point that takes BOTH of the choices a service makes here.
///
/// A service needs it when its recordings show a `@pattern` requirement quoted other than
/// [`unanchored`] — as the model writes it, anchors included, is one such rendering, which
/// `|pattern: &str| pattern.to_string()` states — AND its model marks members `@sensitive`.
/// Everything neither choice covers is the same [`stated_refusal`] body every other entry
/// point here runs, so no combination of the two can drift into a second envelope.
pub fn aws_validation_refusal_withholding_with(
    refusal: &Refusal,
    quote_pattern: impl Fn(&str) -> String,
    withholds_value: impl Fn(&str) -> bool,
) -> Option<Refused> {
    stated_refusal(refusal, quote_pattern, |violation| {
        withholds_value(&violation.path)
    })
}

/// [`aws_validation_refusal`], with the `@pattern` rendering CHOSEN by the caller.
///
/// Everything a service does not choose — which refusals are restated, which violations are
/// touched, the code, the envelope, the member paths and every other requirement — is this
/// one body and the framework's, so the two renderings cannot drift into two envelopes.
/// `quote_pattern` is one of [`crate::validation`]'s two (`unanchored`, or
/// `unanchored_grouping_alternation` for a service whose recordings show a top-level
/// alternation grouped); the CHECK always used the model's pattern as written, so this only
/// changes how the expression is quoted back.
pub fn aws_validation_refusal_with(
    refusal: &Refusal,
    quote_pattern: impl Fn(&str) -> String,
) -> Option<Refused> {
    // Nothing is withheld unless a service says so: the model trait that decides it is not
    // in the runtime schemas, so a caller that does not pass the decision gets the
    // framework's value half on every clause, exactly as before.
    stated_refusal(refusal, quote_pattern, |_| false)
}

/// The refusal a service states for a value one of its own `@pattern`s does not admit,
/// where its recordings show an error OF ITS OWN instead of the envelope above: the
/// parameter the value sat at, and the pattern that value had to match.
///
/// A service reaches for this rather than [`aws_validation_refusal`] when its recordings
/// show that it does not answer `ValidationException` at all for such a value — a model that
/// declares no `ValidationException` cannot answer one — and it chooses only what is its
/// own: `code`, the error it answers (which travels with the message, as everywhere in this
/// module); `claims`, asked about the pattern a violation carries, which decides which of
/// its patterns are stated this way; and `quote_pattern`, the rendering of the expression the
/// message quotes, which the module header above states is the one rendering a service
/// chooses. The SENTENCE is [`parameter_pattern_mismatch`] and stays here, so two services
/// cannot word it differently.
///
/// `None` unless EVERY violation of the refusal is a pattern the service claims, so a
/// refusal that is even partly about another constraint keeps the framework's own envelope
/// rather than being restated as one parameter's.
pub fn parameter_pattern_refusal(
    refusal: &Refusal,
    code: &str,
    claims: impl Fn(&str) -> bool,
    quote_pattern: impl Fn(&str) -> String,
) -> Option<Refused> {
    let Refusal::Invalid(violations) = refusal else {
        return None;
    };
    let claimed: Vec<(&str, String)> = violations
        .iter()
        .map(|violation| match &violation.kind {
            ViolationKind::Pattern(pattern) if claims(pattern) => {
                Some((violation.path.as_str(), quote_pattern(pattern)))
            }
            _ => None,
        })
        .collect::<Option<Vec<_>>>()?;
    // One parameter is named, the first the walk found: the sentence states the parameter
    // whose value was refused, and a refusal naming several would be a different sentence.
    let (member, pattern) = claimed.first()?;
    Some(Refused {
        code: code.to_owned(),
        message: parameter_pattern_mismatch(member, pattern),
    })
}

/// The sentence [`parameter_pattern_refusal`] states: the parameter whose value was refused,
/// and the pattern that value had to match.
///
/// The value itself is WITHHELD — the requirement is the pattern, not the one value that
/// missed it — so the sentence is the same whatever a caller submitted.
#[must_use]
pub fn parameter_pattern_mismatch(member: &str, pattern: &str) -> String {
    parameter_value_mismatch(member, &must_match_pattern(member, pattern))
}

/// The sentence a service states for a parameter whose value it refused: the parameter, and
/// the requirement that value had to satisfy.
///
/// The one sentence [`parameter_pattern_mismatch`] and every other requirement clause below
/// are stated inside, so a service that refuses a value outside a `@pattern` and one that
/// refuses a value outside a `@range` word the two halves of the sentence identically. The
/// value itself is WITHHELD: the requirement is what the parameter admits, not the one value
/// that missed it.
#[must_use]
pub fn parameter_value_mismatch(member: &str, requirement: &str) -> String {
    parameters_value_mismatch(&[member], requirement)
}

/// [`parameter_value_mismatch`] about a COMBINATION of parameters: the requirement holds
/// over several members together, so the sentence names each of them
/// (`The value supplied for parameter 'data' and 'uri' is not valid. …`) and the requirement
/// states what the combination has to satisfy.
///
/// For the refusal a service states where no ONE member's value is wrong — two mutually
/// exclusive members, a member without the companion it requires — which is a requirement of
/// the request rather than of a value, and so cannot be stated as a constraint clause.
#[must_use]
pub fn parameters_value_mismatch(members: &[&str], requirement: &str) -> String {
    format!(
        "The value supplied for parameter {} is not valid. {requirement}",
        quoted_parameters(members)
    )
}

/// The members a refusal names, each quoted, as a sentence lists them: one on its own, two
/// joined by `and`, more than two as a comma-separated list closed by `and`.
fn quoted_parameters(members: &[&str]) -> String {
    let quoted: Vec<String> = members.iter().map(|member| format!("'{member}'")).collect();
    match quoted.split_last() {
        Some((last, [])) => last.clone(),
        Some((last, leading)) => format!("{} and {last}", leading.join(", ")),
        None => String::new(),
    }
}

/// The requirement clause for a value outside its member's `@pattern`: the expression the
/// value had to match, quoted as the service that states it quotes an expression.
#[must_use]
pub fn must_match_pattern(member: &str, pattern: &str) -> String {
    format!("{member} must match pattern {pattern}")
}

/// The requirement clause for a value outside its member's `@pattern` where the service
/// states the requirement WITHOUT quoting the expression — the pattern is named as the one
/// the member requires and nothing of it is echoed.
#[must_use]
pub fn pattern_requirement_unstated(member: &str) -> String {
    format!("{member} does not match the required pattern")
}

/// The requirement clause for a value above its member's `@range` maximum: the bound the
/// value had to stay at or below.
#[must_use]
pub fn must_be_at_most(member: &str, bound: &str) -> String {
    format!("{member} must be smaller than or equal to {bound}")
}

/// The requirement clause for a value below its member's `@range` minimum: the bound the
/// value had to reach.
#[must_use]
pub fn must_be_at_least(member: &str, bound: &str) -> String {
    format!("{member} must be greater than or equal to {bound}")
}

/// The requirement clause for a value outside an enumerated type's value set: the set the
/// value had to come from, each value quoted, in the order the model declares them.
///
/// A service whose recordings state that set in another order hands the violation through
/// [`in_sorted_enum_order`] first, which is where an order of its own is chosen; this states
/// the set it is given.
#[must_use]
pub fn must_be_one_of(member: &str, values: &[String]) -> String {
    let quoted: Vec<String> = values.iter().map(|value| format!("'{value}'")).collect();
    format!("{member} must be one of [{}]", quoted.join(", "))
}

/// The segment the framework's member paths mark a step from a collection into its element
/// with (`aws_smithy_schema_server::constraints::path::element`), which the API spells as an
/// INDEX on the collection instead.
const COLLECTION_STEP: &str = "member";

/// `path` as the API spells the parameter it names: the step the framework's walk takes from
/// a COLLECTION into one of its elements is not a parameter of the API, so a list element is
/// addressed by its zero-based index on the member that holds it
/// (`distributions.1.member.amiDistributionConfiguration.name` becomes
/// `distributions[0].amiDistributionConfiguration.name`) and a map entry by its key alone
/// (`amiTags.owner.member` becomes `amiTags.owner`).
///
/// The framework reports a violation at the path its validating walk builds, which spells an
/// element the way the walk steps into it — a one-based position (or the entry's key) and the
/// collection step — and no API has such a member: a caller fixing the request addresses the
/// element by its index on the member that holds it. Every other segment is left exactly as
/// the walk spelled it.
///
/// The collection step is recognised by its spelling, which is the only thing a dotted path
/// carries: a STRUCTURE member of that name, nested under another member, is indistinguishable
/// from it and is folded the same way. The framework's own paths have that ambiguity, and the
/// first segment is never read as a step, so a top-level member of that name is untouched.
#[must_use]
pub fn api_parameter_path(path: &str) -> String {
    let mut steps: Vec<String> = Vec::new();
    let mut segments = path.split('.').peekable();
    while let Some(segment) = segments.next() {
        // The walk steps into an element as `<position-or-key>.member`, and only from a
        // collection it already named, so the first segment of a path is never such a step.
        if steps.is_empty() || segments.peek() != Some(&COLLECTION_STEP) {
            steps.push(segment.to_string());
            continue;
        }
        segments.next();
        match segment
            .parse::<usize>()
            .ok()
            .filter(|position| *position >= 1)
        {
            // A list element: its index on the member that holds it, counted from zero.
            Some(position) => {
                if let Some(holder) = steps.last_mut() {
                    holder.push_str(&format!("[{}]", position - 1));
                }
            }
            // A map entry: the key the caller chose, as the request carried it.
            None => steps.push(segment.to_string()),
        }
    }
    steps.join(".")
}

/// The ONE body behind every entry point above: the framework's envelope over the
/// framework's clauses, with the two renderings this module owns applied — the `@pattern`
/// quoting the caller chose, and the value half dropped from a clause that states none (a
/// collection's `@length`, or a violation the caller withholds the value of).
///
/// `withholds_value` is asked about the violation rather than about its path, because the
/// two things a service decides it by — which MEMBERS its model marks `@sensitive` and which
/// constraint CLASSES its recordings state without a value — are both properties of the
/// violation; the path-keyed entry points above pass the path out of it.
///
/// LINEAR in the number of violations: each clause is decided BEFORE the envelope is
/// rendered, so no clause is rewritten inside an already-rendered message (which would
/// rescan and reallocate the whole message once per violation), and the count the envelope
/// states is bounded by [`restated_violations`].
fn stated_refusal(
    refusal: &Refusal,
    quote_pattern: impl Fn(&str) -> String,
    withholds_value: impl Fn(&Violation) -> bool,
) -> Option<Refused> {
    let Refusal::Invalid(violations) = refusal else {
        return None;
    };
    // The envelope and every clause are the framework's; a COLLECTION's length clause
    // then drops its value half, which the framework has no way to leave out (a clause
    // carrying no value reads `Value null at …`, which is how an ABSENT member is stated).
    let clauses: Vec<String> = restated_violations(violations, &quote_pattern)
        .iter()
        .map(|violation| {
            let stated = if withholds_value(violation) {
                valueless_clause(violation)
            } else {
                collection_length_clause(violation)
            };
            stated.unwrap_or_else(|| violation.message())
        })
        .collect();
    Some(Refused {
        code: VALIDATION_EXCEPTION.to_owned(),
        message: envelope_over(&clauses)?,
    })
}

/// The violations one message restates, each with its `@pattern` requoted.
///
/// BOUNDED by the same ceiling the framework's own walk collects under
/// ([`crate::validation::MAX_VIOLATIONS`]), so the clause count — and with it the length of
/// the message and the work of rendering it — is a constant of this crate rather than
/// something one request chooses. The walk never hands over more than that many, so the
/// bound only states locally what is already true of every caller.
fn restated_violations(
    violations: &[Violation],
    quote_pattern: &dyn Fn(&str) -> String,
) -> Vec<Violation> {
    violations
        .iter()
        .take(crate::validation::MAX_VIOLATIONS)
        .map(|violation| requoted(violation, quote_pattern))
        .collect()
}

/// What a clause opens the element shape's constraint set with, when the set is stated at
/// the collection member the elements belong to.
const ELEMENT_SET_OPEN: &str = "Member must satisfy constraint: [";

/// State `refusal` as AWS states it for a service that reports a constraint broken by a
/// collection's ELEMENTS at the COLLECTION MEMBER, or `None` to keep the framework's own
/// wording.
///
/// This is [`aws_validation_refusal_with`] plus one grouping rule:
///
/// * a violation at a list ELEMENT (`<collection>.<position>.member`,
///   [`crate::validation::list_of_element`]) is stated at `<collection>` — no position and
///   no `member` step — with every requirement the ELEMENT shape declares nested inside
///   `Member must satisfy constraint: [ \u2026 ]`, and with no value: a collection has no scalar
///   rendering to quote, which is the same half a collection's own `@length` clause drops.
///   The set is the element shape's whole set, not just the constraint that happened to
///   fail, so one request breaking two of an element's constraints is ONE validation error
///   about one member; two offending elements of one collection are one clause too.
/// * a violation on the collection ITSELF (its `@length`, its `@uniqueItems`), or on a
///   member inside a structure element, keeps its own clause and its own path.
///
/// `input_schemas` are the operation input shapes of the calling service, which the element
/// sets are read off: codegen attaches a collection member's element constraints to that
/// member's schema as an [`ElementConstraintsTrait`], and the refusal carries only the
/// constraints that FAILED, so a set read off the violations would be missing the ones the
/// value satisfied. Only the TOP-LEVEL members of those shapes are looked up, and a member
/// path two of them declare with DIFFERENT element sets keeps the framework's own clause:
/// the refusal does not say which operation it came from, and a guessed set would not be
/// known to be true of the request.
///
/// `quote_pattern` chooses the `@pattern` rendering exactly as it does for
/// [`aws_validation_refusal_with`] — `str::to_string` for a service whose refusals quote
/// the expression the model declares, [`crate::validation::unanchored`] for one whose
/// recordings drop the anchors. The CHECK always used the model's pattern as written.
///
/// THE HOME for this grouping, under the same rule as [`pattern_mismatch`] and
/// [`enum_value_not_found`]: the service crates below each carry their own inlined copy of
/// it, written before this module had one, and are to be MIGRATED onto this entry point
/// rather than copied again (`grep -rln 'Member must satisfy constraint:' services/*/src`
/// lists them): opensearch-amazonopensearchservice,
/// bedrock-amazonbedrockcontrolplaneservice,
/// bedrockagentcorecontrol-amazonbedrockagentcorecontrol,
/// lambdacore-lambdacoreapiservice, rum-rum, identitystore-awsidentitystore,
/// acm-certificatemanager, codeguruprofiler-codeguruprofiler,
/// billingconductor-awsbillingconductor,
/// licensemanagerlinuxsubscriptions-licensemanagerlinuxsubscriptions, ssmsap-ssmsap and
/// workspacesinstances-eucmifrontendapiservice. Each migration is that service's own
/// change, because each has to be re-checked against its own recordings: they do not agree
/// on the value half of the clause or on how the `@pattern` is quoted.
pub fn aws_validation_refusal_over_elements(
    refusal: &Refusal,
    input_schemas: &[&'static Schema],
    quote_pattern: impl Fn(&str) -> String,
) -> Option<Refused> {
    aws_validation_refusal_over_elements_quoting(refusal, input_schemas, quote_pattern, |_| None)
}

/// [`aws_validation_refusal_over_elements_quoting`], with the nested REQUIREMENT a grouped
/// clause states decided by the caller.
///
/// The other half those recordings do not agree on: WHAT an element shape's constraint set
/// reads as. The set this module reads off the handed-over schemas is the constraints the
/// model declares on the element shape, looked up by the member path the clause is stated at
/// — and a service whose recordings show more than that (every element of a list that is not
/// `@sparse` also has to be PRESENT, which several services' refusals state beside the
/// constraint the value broke), or whose list members are nested inside structures the
/// handed-over schemas name under their own member names, states the requirement itself.
/// `element_set` is asked for it by that member path, and `None` falls back to the set this
/// module reads, so a service states only the collections whose set is its own.
///
/// The requirement is nested with [`element_set_requirement`] by whoever builds it;
/// everything else is [`aws_validation_refusal_over_elements_quoting`]'s — the same
/// grouping, the same value half, the same envelope and the same clauses for every violation
/// that is not a collection element's.
pub fn aws_validation_refusal_over_elements_stating(
    refusal: &Refusal,
    quote_pattern: impl Fn(&str) -> String,
    quoted_collection: impl Fn(&str) -> Option<String>,
    element_set: impl Fn(&str) -> Option<String>,
) -> Option<Refused> {
    grouped_refusal(
        refusal,
        &[],
        &quote_pattern,
        &quoted_collection,
        &|_| false,
        &element_set,
    )
}

/// [`aws_validation_refusal_over_elements`], with the COLLECTION a grouped clause quotes
/// decided by the caller.
///
/// The one half those recordings do not agree on: a service whose clause states the
/// collection its elements belong to passes the value that member held, which no violation
/// carries and which only the request can answer — `quoted_collection` is asked for it, by
/// the member path the clause is stated at, and `None` keeps the valueless clause
/// [`aws_validation_refusal_over_elements`] states. The collection is rendered the way AWS
/// renders one (`crate::validation::submitted_lists`, which collects exactly that from the
/// request under bounds of its own). Everything else is that function's: the same grouping,
/// the same element sets read off the same schemas, the same envelope.
pub fn aws_validation_refusal_over_elements_quoting(
    refusal: &Refusal,
    input_schemas: &[&'static Schema],
    quote_pattern: impl Fn(&str) -> String,
    quoted_collection: impl Fn(&str) -> Option<String>,
) -> Option<Refused> {
    // Nothing is withheld unless a service says so, exactly as for the ungrouped entry
    // points: the model trait that decides it is not in the runtime schemas.
    grouped_refusal(
        refusal,
        input_schemas,
        &quote_pattern,
        &quoted_collection,
        &|_| false,
        &|_| None,
    )
}

/// [`aws_validation_refusal_over_elements`], with the members whose value the refusal
/// WITHHOLDS chosen by the caller — the one entry point that takes BOTH halves a service
/// decides here: the grouping over a collection's elements, and the value half of every
/// OTHER clause.
///
/// `withholds_value` is asked about a violation's member path and a clause it answers `true`
/// for states no value at all — neither the submitted bytes nor the `null` the framework
/// writes for an absent member — which is [`aws_validation_refusal_withholding`]'s rule, so a
/// service whose refusals quote no value anywhere passes `|_| true`. A grouped clause states
/// no value either way, so the two rules compose rather than overlap. Everything neither
/// choice covers is the same body [`aws_validation_refusal_over_elements_quoting`] runs, so no
/// combination can drift into a second envelope.
pub fn aws_validation_refusal_over_elements_withholding(
    refusal: &Refusal,
    input_schemas: &[&'static Schema],
    quote_pattern: impl Fn(&str) -> String,
    withholds_value: impl Fn(&str) -> bool,
) -> Option<Refused> {
    grouped_refusal(
        refusal,
        input_schemas,
        &quote_pattern,
        &|_| None,
        &withholds_value,
        &|_| None,
    )
}

/// [`aws_validation_refusal_over_elements_quoting`], with the members whose value the
/// refusal WITHHOLDS chosen by the caller too — the entry point for a service whose
/// recordings show BOTH the collection a grouped clause quotes and a member stated without
/// the value it submitted.
///
/// The two choices are the ones [`aws_validation_refusal_over_elements_quoting`] and
/// [`aws_validation_refusal_over_elements_withholding`] take one each, composed over the one
/// [`grouped_refusal`] body: a grouped clause quotes the collection `quoted_collection`
/// answers for and states no value where it answers `None`, and every OTHER clause drops its
/// value half where `withholds_value` answers `true` for its member path. A grouped clause is
/// never asked the second question, so the two rules compose rather than overlap, and
/// everything neither covers — the grouping, the element sets read off `input_schemas`, the
/// envelope, the member paths, the `@pattern` rendering and every other requirement — is that
/// same body's and the framework's.
pub fn aws_validation_refusal_over_elements_quoting_withholding(
    refusal: &Refusal,
    input_schemas: &[&'static Schema],
    quote_pattern: impl Fn(&str) -> String,
    quoted_collection: impl Fn(&str) -> Option<String>,
    withholds_value: impl Fn(&str) -> bool,
) -> Option<Refused> {
    grouped_refusal(
        refusal,
        input_schemas,
        &quote_pattern,
        &quoted_collection,
        &withholds_value,
        &|_| None,
    )
}

/// [`aws_validation_refusal_over_elements_stating`], with the members whose value the
/// refusal WITHHOLDS chosen by the caller too — the entry point for a service that states
/// BOTH halves at once: the nested requirement of a grouped clause, and the value half of
/// every other clause.
///
/// The two choices are the ones [`aws_validation_refusal_over_elements_stating`] and
/// [`aws_validation_refusal_over_elements_withholding`] take one each, composed over the one
/// [`grouped_refusal`] body. A service needs both when its list members sit INSIDE
/// structures its schemas name under their own member names — so the set has to be stated
/// rather than looked up by the member path the clause is stated at — AND its recordings
/// quote no value in any clause, which `|_| true` states. A grouped clause states no value
/// either way, so the two rules compose rather than overlap, and everything neither covers —
/// the grouping, the envelope, the member paths, the `@pattern` rendering and every other
/// requirement — is that same body's and the framework's.
pub fn aws_validation_refusal_over_elements_stating_withholding(
    refusal: &Refusal,
    quote_pattern: impl Fn(&str) -> String,
    withholds_value: impl Fn(&str) -> bool,
    element_set: impl Fn(&str) -> Option<String>,
) -> Option<Refused> {
    grouped_refusal(
        refusal,
        &[],
        &quote_pattern,
        &|_| None,
        &withholds_value,
        &element_set,
    )
}

/// The constraints the ELEMENT shape of the collection member `collection` declares, in the
/// order a clause states them — for a service that renders the requirement SENTENCES itself
/// instead of delegating the whole refusal to [`aws_validation_refusal_over_elements`].
///
/// A service needs this when the clauses it states around the nested set are its own: one
/// whose refusals withhold the value a PRESENT member submitted while still stating the
/// `null` an ABSENT one is reported with cannot express that per member path, which is the
/// only decision the grouped entry points above take. It gets the same reading of the same
/// schemas they use, so the set cannot differ between the two.
///
/// `shapes` are shapes of the calling service whose own members are looked up — its
/// operation inputs, and the structures its inputs nest, since a member schema carries its
/// own traits and not the members of the shape it targets. `collection` is the member as a
/// violation path spells it (lowerCamel, [`aws_smithy_schema_server::constraints::path`]),
/// so a nested member is looked up by its own name in the shape that declares it.
///
/// `None` when no handed-over shape declares an element constraint set for that member, and
/// `None` when two of them declare DIFFERENT sets: the refusal does not say which operation
/// it came from, so a guessed set would not be known to be true of the request. A
/// `@pattern` comes back as the MODEL writes it, which leaves the rendering of the
/// expression to the caller exactly as the grouped entry points leave it.
#[must_use]
pub fn element_constraint_set(
    shapes: &[&'static Schema],
    collection: &str,
) -> Option<Vec<ViolationKind>> {
    let mut declared = shapes
        .iter()
        .filter_map(|schema| element_constraints_declared(schema, collection));
    let set = declared.next()?;
    declared.all(|other| other == set).then_some(set)
}

/// The requirement a clause states at a COLLECTION member for a constraint its ELEMENTS
/// broke: the element shape's whole declared set, nested inside
/// `Member must satisfy constraint: [ … ]`.
///
/// The wrapper lives here and nowhere else, so a service that states the clause itself
/// ([`element_constraint_set`]) and one that delegates the whole refusal
/// ([`aws_validation_refusal_over_elements`]) cannot word it differently. `requirements` are
/// the element shape's requirement sentences, in the order the clause states them.
#[must_use]
pub fn element_set_requirement(requirements: &[String]) -> String {
    format!("{ELEMENT_SET_OPEN}{}]", requirements.join(", "))
}

/// What a clause opens the VALUE shape's constraint set with, when the set is stated at the
/// MAP member whose values it is about — the map counterpart of [`ELEMENT_SET_OPEN`], and
/// the sibling of the `Map keys must satisfy constraint: …` [`map_key_refusal`] states.
const MAP_VALUE_SET_OPEN: &str = "Map value must satisfy constraint: [";

/// The requirement a clause states at a MAP member for a constraint its VALUES broke: the
/// value shape's whole declared set, nested inside
/// `Map value must satisfy constraint: [ … ]`.
///
/// The wrapper lives here for the same reason [`element_set_requirement`]'s does: the
/// grouping needs the REQUEST (the map a clause quotes is the value that member held, which
/// no violation carries), so it is made per service, while the sentence around it is one
/// text and cannot be allowed to drift per crate. `requirements` are the value shape's
/// requirement sentences — [`requirement`] over the constraints
/// [`element_constraint_set`] reads off the model — in the order the clause states them.
///
/// THE HOME for this sentence, under the same rule as [`element_set_requirement`]:
/// `sagemaker-sagemaker`, `aiops-aiops` and `pipes-pipes` each carry their own inlined copy,
/// written before this module had one, and are to be MIGRATED onto this one rather than
/// copied again.
#[must_use]
pub fn map_value_set_requirement(requirements: &[String]) -> String {
    format!("{MAP_VALUE_SET_OPEN}{}]", requirements.join(", "))
}

/// The ONE body behind both grouped entry points: the framework's envelope over the
/// framework's clauses, with a violation at a list ELEMENT stated at the collection member
/// carrying the element shape's whole set, and the value half dropped from a clause that
/// states none (a collection's `@length`, or a member the caller withholds the value of).
///
/// LINEAR in the number of violations, which the walk itself bounds
/// ([`restated_violations`]): each clause is decided once, and an identical clause is
/// recognised through a set rather than by rescanning the clauses already collected.
fn grouped_refusal(
    refusal: &Refusal,
    input_schemas: &[&'static Schema],
    quote_pattern: &dyn Fn(&str) -> String,
    quoted_collection: &dyn Fn(&str) -> Option<String>,
    withholds_value: &dyn Fn(&str) -> bool,
    element_set: &dyn Fn(&str) -> Option<String>,
) -> Option<Refused> {
    let Refusal::Invalid(violations) = refusal else {
        return None;
    };
    let mut clauses: Vec<String> = Vec::new();
    // Dedupe through a set rather than by scanning what is already collected, so the cost
    // stays linear in the clause count.
    let mut stated: HashSet<String> = HashSet::new();
    for violation in &restated_violations(violations, quote_pattern) {
        let clause = list_of_element(&violation.path)
            .and_then(|collection| {
                // The requirement the CALLER states for that collection comes first, and
                // the set this module reads off the schemas stands for every collection it
                // does not state (see
                // [`aws_validation_refusal_over_elements_stating`]).
                let quoted = quoted_collection(collection);
                match element_set(collection) {
                    Some(requirement) => Some(stated_element_clause(
                        collection,
                        quoted.as_deref(),
                        &requirement,
                    )),
                    None => element_set_clause(
                        collection,
                        input_schemas,
                        quote_pattern,
                        quoted.as_deref(),
                    ),
                }
            })
            .or_else(|| {
                withholds_value(&violation.path)
                    .then(|| valueless_clause(violation))
                    .flatten()
            })
            .or_else(|| collection_length_clause(violation))
            .unwrap_or_else(|| violation.message());
        // An identical clause is stated once: the member is named once however many of
        // its elements the walk reported.
        if stated.insert(clause.clone()) {
            clauses.push(clause);
        }
    }
    Some(Refused {
        code: VALIDATION_EXCEPTION.to_owned(),
        message: envelope_over(&clauses)?,
    })
}

/// `violation` with its `@pattern` requirement quoted the way the caller chose, and
/// everything else untouched.
///
/// The ONE place that mapping is made, so the two entry points cannot drift in how an
/// expression is quoted back.
fn requoted(violation: &Violation, quote_pattern: &dyn Fn(&str) -> String) -> Violation {
    match &violation.kind {
        ViolationKind::Pattern(pattern) => Violation {
            path: violation.path.clone(),
            kind: ViolationKind::Pattern(quote_pattern(pattern)),
            value: violation.value.clone(),
            sensitive: violation.sensitive,
        },
        _ => violation.clone(),
    }
}

/// The clause stated at `collection` for a constraint its elements broke, or `None` when
/// the handed-over schemas declare no single element constraint set for that member.
///
/// `quoted` is the collection as the clause quotes it, and `None` states no value at all —
/// see [`aws_validation_refusal_over_elements_quoting`].
fn element_set_clause(
    collection: &str,
    input_schemas: &[&'static Schema],
    quote_pattern: &dyn Fn(&str) -> String,
    quoted: Option<&str>,
) -> Option<String> {
    let mut declared = input_schemas
        .iter()
        .filter_map(|schema| element_requirements(schema, collection, quote_pattern));
    let set = declared.next()?;
    if declared.any(|other| other != set) {
        return None;
    }
    Some(stated_element_clause(
        collection,
        quoted,
        &element_set_requirement(&set),
    ))
}

/// The clause stated at `collection` for `requirement`, the nested requirement its elements
/// have to satisfy: the ONE place a grouped clause is spelled, whether the set came off the
/// schemas or from the caller.
///
/// The value half is the framework's own spelling of one, so a grouped clause reads like a
/// clause about a member the walk reported; `quoted` is the collection as the clause quotes
/// it, and `None` states no value at all.
fn stated_element_clause(collection: &str, quoted: Option<&str>, requirement: &str) -> String {
    let stated_value = match quoted {
        Some(value) => format!("Value '{value}' at"),
        None => "Value at".to_string(),
    };
    format!("{stated_value} '{collection}' failed to satisfy constraint: {requirement}")
}

/// Every requirement the ELEMENT shape of `input_schema`'s `collection` member declares, in
/// the order a clause states them, or `None` when that member is not a collection of
/// constrained scalars.
///
/// Every sentence is the framework's own ([`requirement`]) over the constraints
/// [`element_constraints_declared`] read off the model, so a requirement nested into a
/// collection clause reads exactly like one a reported member gets.
fn element_requirements(
    input_schema: &Schema,
    collection: &str,
    quote_pattern: &dyn Fn(&str) -> String,
) -> Option<Vec<String>> {
    let declared = element_constraints_declared(input_schema, collection)?;
    Some(
        declared
            .into_iter()
            .map(|kind| match kind {
                // The CHECK used the model's pattern as written; only the rendering of the
                // expression is the caller's choice.
                ViolationKind::Pattern(pattern) => {
                    requirement(ViolationKind::Pattern(quote_pattern(&pattern)))
                }
                other => requirement(other),
            })
            .collect(),
    )
}

/// Every constraint the ELEMENT shape of `input_schema`'s `collection` member declares, in
/// the order a clause states them, or `None` when that member is not a collection of
/// constrained scalars.
///
/// The ORDER is the one AWS's element-constraint clauses state: the `@length` maximum, then
/// its minimum, then the numeric bounds the same way, then the `@pattern`, then an
/// enumerated value set. A `@length` with no declared minimum states `0`, the smallest
/// length a Smithy `@length` can bound. A `@pattern` is carried as the MODEL writes it, so
/// whoever states it chooses the rendering of the expression.
fn element_constraints_declared(
    input_schema: &Schema,
    collection: &str,
) -> Option<Vec<ViolationKind>> {
    let member = input_schema.members().iter().find(|member| {
        member
            .member_name()
            .is_some_and(|name| constraints::path::member(name) == collection)
    })?;
    let traits = constraint_trait::<ElementConstraintsTrait>(
        member.traits()?,
        &ElementConstraintsTrait::TRAIT_ID,
    )?
    .inner();

    let mut declared: Vec<ViolationKind> = Vec::new();
    if let Some(length) = constraint_trait::<LengthTrait>(traits, &LengthTrait::TRAIT_ID) {
        if let Some(max) = length.max {
            declared.push(ViolationKind::MaxLength(max));
        }
        declared.push(ViolationKind::MinLength(length.min.unwrap_or(0)));
    }
    if let Some(range) = constraint_trait::<RangeTrait>(traits, &RangeTrait::TRAIT_ID) {
        if let Some(max) = range.max {
            declared.push(ViolationKind::MaxValue(rendered_bound(max)));
        }
        if let Some(min) = range.min {
            declared.push(ViolationKind::MinValue(rendered_bound(min)));
        }
    }
    if let Some(pattern) = constraint_trait::<PatternTrait>(traits, &PatternTrait::TRAIT_ID) {
        declared.push(ViolationKind::Pattern(pattern.regex.to_owned()));
    }
    if let Some(allowed) = constraint_trait::<EnumTrait>(traits, &EnumTrait::TRAIT_ID) {
        let values: Vec<String> = allowed.values.iter().map(|value| (*value).to_owned()).collect();
        declared.push(ViolationKind::EnumValue(values));
    }
    (!declared.is_empty()).then_some(declared)
}

/// One constraint trait off a trait map, downcast to the type carrying its bounds.
fn constraint_trait<'map, T: 'static>(traits: &'map TraitMap, id: &ShapeId) -> Option<&'map T> {
    traits.get(id)?.as_any().downcast_ref::<T>()
}

/// A `@range` bound as a requirement displays it: an integral bound without a trailing
/// `.0`, a fractional one with its decimals — the rendering the framework's own clause
/// carries, so a bound read off the model and one read off a violation agree.
///
/// `{bound:.0}` rather than a cast through `i64`, which SATURATES: a bound past `i64`'s
/// magnitude would otherwise advertise `i64::MAX` instead of what the model declares.
fn rendered_bound(bound: f64) -> String {
    if bound.fract() == 0.0 && bound.is_finite() {
        format!("{bound:.0}")
    } else {
        format!("{bound}")
    }
}

/// The framework's envelope over clauses this module stated rather than over violations.
///
/// The count word and the separator are the framework's: they are read back out of
/// [`envelope`]'s own rendering over as many placeholder violations as there are clauses,
/// so neither is spelled here and a grouped refusal cannot drift from an ungrouped one.
/// `None` for no clause at all, exactly as [`envelope`] answers for no violation.
///
/// `pub` for the same reason [`requirement`] is: a service whose clauses this module's
/// whole-refusal entry points cannot state words them itself and still has to put them in
/// AWS's envelope, and an envelope re-spelled per crate is how `1 validation error
/// detected: ` comes to differ between two services.
#[must_use]
pub fn envelope_over(clauses: &[String]) -> Option<String> {
    let placeholder = Violation {
        path: String::new(),
        kind: ViolationKind::MissingRequiredMember,
        value: None,
        sensitive: false,
    };
    let rendered = envelope(&vec![placeholder.clone(); clauses.len()])?;
    let stated = placeholder.message();
    let (prefix, rest) = rendered.split_once(&stated)?;
    // With every placeholder identical, whatever sits between the first two is the
    // separator; one clause needs none.
    let separator = rest.split_once(&stated).map_or("", |(between, _)| between);
    Some(format!("{prefix}{}", clauses.join(separator)))
}

#[cfg(test)]
mod tests {
    use super::{
        api_parameter_path, array_too_short, aws_validation_refusal,
        aws_validation_refusal_over_elements,
        aws_validation_refusal_over_elements_stating_withholding, aws_validation_refusal_with,
        aws_validation_refusal_withholding, aws_validation_refusal_withholding_violations,
        aws_validation_refusal_withholding_with, element_set_requirement, enum_value_bucket,
        enum_value_not_found,
        envelope_over, float_value_text, hashed_enum_order, in_float_value_rendering,
        in_hashed_enum_order, in_pattern_first_order, in_stated_enum_order,
        instance_type_mismatch, map_key_refusal,
        map_key_refusal_quoting, map_key_requirements_refusal, must_be_at_least, must_be_at_most,
        must_be_one_of,
        must_match_pattern, parameter_pattern_mismatch, parameter_pattern_refusal,
        parameter_value_mismatch, parameters_value_mismatch, pattern_constraint_report,
        pattern_mismatch, pattern_requirement_unstated, property_count_refusal,
        quoting_the_submitted_collection, quoting_the_whole_submitted_value, requirement,
        stated_enum_value_set, string_too_long, string_too_short, StatedEnumValueSets,
    };
    use crate::validation::unanchored_grouping_alternation;
    use aws_smithy_schema::{Schema, ShapeId, ShapeType, TraitMap};
    use aws_smithy_schema_server::constraints::{
        ElementConstraintsTrait, LengthTrait, PatternTrait,
    };
    use aws_smithy_schema_server::{envelope, Violation, ViolationKind};
    use smithy_server_runtime::Refusal;
    use std::sync::LazyLock;

    /// One violation of `kind` at `path`, for the value a request sent.
    fn violation(path: &str, kind: ViolationKind, value: &str) -> Violation {
        Violation {
            path: path.to_owned(),
            kind,
            value: Some(value.to_owned()),
            sensitive: false,
        }
    }

    /// RULE: the clauses come out with the members in the order the walk found them, and
    /// within one member its `@pattern` requirement before that member's others.
    ///
    /// The rows span what the order generalises over: a member whose pattern was reported
    /// after another of its requirements and one where it came first, a member with no
    /// pattern at all, several members interleaved, and the empty refusal.
    #[test]
    fn a_members_pattern_requirement_is_stated_before_its_others() {
        /// The `(path, is a pattern)` pairs one ordering produced, as a reader sees them.
        fn stated(violations: &[Violation]) -> Vec<(&str, bool)> {
            in_pattern_first_order(violations)
                .into_iter()
                .map(|violation| {
                    (
                        violation.path.as_str(),
                        matches!(violation.kind, ViolationKind::Pattern(_)),
                    )
                })
                .collect()
        }

        let pattern = |path: &str| {
            violation(
                path,
                ViolationKind::Pattern("^[a-z]+$".to_owned()),
                "NOPE",
            )
        };
        let length = |path: &str| violation(path, ViolationKind::MinLength(1), "");
        let range = |path: &str| violation(path, ViolationKind::MaxValue("1".to_owned()), "2.5");

        /// One row: the case, the violations the walk collected, and the order they come
        /// out in.
        type Row = (&'static str, Vec<Violation>, Vec<(&'static str, bool)>);

        let cases: [Row; 5] = [
            (
                "a member whose length was reported before its pattern",
                vec![length("name"), pattern("name")],
                vec![("name", true), ("name", false)],
            ),
            (
                "a member whose pattern was reported first already",
                vec![pattern("name"), length("name")],
                vec![("name", true), ("name", false)],
            ),
            (
                "a member with no pattern at all",
                vec![range("threshold")],
                vec![("threshold", false)],
            ),
            (
                "two members, each with two requirements",
                vec![
                    length("first"),
                    range("second"),
                    pattern("second"),
                    pattern("first"),
                ],
                vec![
                    ("first", true),
                    ("first", false),
                    ("second", true),
                    ("second", false),
                ],
            ),
            ("no violation at all", Vec::new(), Vec::new()),
        ];

        for (case, violations, expected) in cases {
            let actual = stated(&violations);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the order {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: a floating-point value is quoted back with a fractional part or an exponent,
    /// in scientific notation outside the plain-decimal band.
    ///
    /// The rows differ along what the rule generalises over: the magnitude (far below the
    /// band, just below it, inside it, at and past its upper end), the sign, whether the
    /// value is whole or fractional, how many digits it carries, and zero.
    #[test]
    fn a_floating_point_value_is_quoted_the_way_a_service_writes_one() {
        let rows: [(f64, &str); 14] = [
            (0.0, "0.0"),
            (1.0, "1.0"),
            (-5.0, "-5.0"),
            (0.5, "0.5"),
            (1.000001, "1.000001"),
            (-0.000001, "-1.0E-6"),
            (-1.5e-7, "-1.5E-7"),
            (0.001, "0.001"),
            (-0.0009, "-9.0E-4"),
            (9999999.0, "9999999.0"),
            (1e7, "1.0E7"),
            (12345678.0, "1.2345678E7"),
            (-1e30, "-1.0E30"),
            (2.5e-12, "2.5E-12"),
        ];
        for (value, expected) in rows {
            let rendered = float_value_text(value);
            assert_eq!(
                rendered, expected,
                "case {value}: expected {expected:?}, got {rendered:?}"
            );
        }
    }

    /// RULE: the value half of a clause about a COLLECTION is replaced by the collection
    /// the caller submitted, and only there: a clause whose stored value is already the
    /// caller's own bytes, and a collection the walk could not render, keep what they had.
    ///
    /// The rows span both collection summaries the walk writes, both `@length` bounds, a
    /// value that is not a summary at all, a violation of another class, and an echo the
    /// walk could not produce.
    #[test]
    fn a_clause_about_a_collection_quotes_the_collection_that_was_submitted() {
        use crate::validation::SubmittedEcho;

        // (case, the violation's kind, its stored value, the echo, the value stated)
        let rows: [(&str, ViolationKind, &str, SubmittedEcho, &str); 6] = [
            (
                "a list over its maximum",
                ViolationKind::MaxLength(1),
                "2 items",
                SubmittedEcho::Rendered("[a, b]".to_string()),
                "[a, b]",
            ),
            (
                "an empty list under its minimum",
                ViolationKind::MinLength(1),
                "0 items",
                SubmittedEcho::Rendered("[]".to_string()),
                "[]",
            ),
            (
                "a map over its maximum",
                ViolationKind::MaxLength(1),
                "2 entries",
                SubmittedEcho::Rendered("{a=1, b=2}".to_string()),
                "{a=1, b=2}",
            ),
            (
                "a value that is the caller's own bytes",
                ViolationKind::MaxLength(1),
                "a-name",
                SubmittedEcho::Rendered("a-name".to_string()),
                "a-name",
            ),
            (
                "a violation of another class",
                ViolationKind::Pattern("[a-z]+".to_string()),
                "2 items",
                SubmittedEcho::Rendered("[a, b]".to_string()),
                "2 items",
            ),
            (
                "a collection the walk could not render",
                ViolationKind::MaxLength(1),
                "2 items",
                SubmittedEcho::Unrendered,
                "2 items",
            ),
        ];

        for (case, kind, stored, echo, expected) in rows {
            let stated = quoting_the_submitted_collection(violation("tags", kind, stored), echo);
            assert_eq!(
                stated.value.as_deref(),
                Some(expected),
                "case {case:?}: expected {expected:?}, got {:?}",
                stated.value
            );
        }
    }

    /// RULE: a clause whose value half the framework ELIDED states the whole value the
    /// caller submitted, and only there: a clause the framework wrote from something else
    /// keeps what it had.
    ///
    /// The rows span the forms the framework's rendering takes — the value itself, a prefix
    /// plus the marker naming its byte count — beside the ones that are not that rendering
    /// at all: a collection's size summary, an echo the walk could not produce, and a stored
    /// value that is a prefix of the submitted one WITHOUT the marker.
    #[test]
    fn a_clause_the_framework_elided_quotes_the_whole_submitted_value() {
        use crate::validation::SubmittedEcho;

        let long = "x".repeat(300);
        let elided = format!("{}... [{} bytes]", &long[..256], long.len());
        // (case, the violation's stored value, the echo, the value stated)
        let rows: [(&str, &str, SubmittedEcho, &str); 5] = [
            (
                "a value the framework elided",
                elided.as_str(),
                SubmittedEcho::Rendered(long.clone()),
                long.as_str(),
            ),
            (
                "a value short enough to be stored whole",
                "a-name",
                SubmittedEcho::Rendered("a-name".to_string()),
                "a-name",
            ),
            (
                "a collection's size summary, which is no elision of a value",
                "2 items",
                SubmittedEcho::Rendered("[a, b]".to_string()),
                "2 items",
            ),
            (
                "an echo the walk could not produce",
                elided.as_str(),
                SubmittedEcho::Unrendered,
                elided.as_str(),
            ),
            (
                "a prefix carrying no byte-count marker",
                "xxx",
                SubmittedEcho::Rendered(long.clone()),
                "xxx",
            ),
        ];

        for (case, stored, echo, expected) in rows {
            let stated = quoting_the_whole_submitted_value(
                violation("description", ViolationKind::MaxLength(8), stored),
                &echo,
            );
            assert_eq!(
                stated.value.as_deref(),
                Some(expected),
                "case {case:?}: expected a value of {} bytes, got {:?} bytes",
                expected.len(),
                stated.value.as_deref().map(str::len)
            );
        }
    }

    /// RULE: the rendering reaches a clause only for a member the CALLER says is floating
    /// point, and only the value half of that clause changes.
    #[test]
    fn only_a_floating_point_members_clause_is_requoted() {
        let float = violation(
            "inferenceConfig.topP",
            ViolationKind::MinValue("0".to_owned()),
            "-0.000001",
        );
        let integral = violation("maxResults", ViolationKind::MinValue("1".to_owned()), "0");
        let is_float = |path: &str| path == "inferenceConfig.topP";
        let requoted = in_float_value_rendering(&float, is_float);
        assert_eq!(
            requoted.message(),
            "Value '-1.0E-6' at 'inferenceConfig.topP' failed to satisfy constraint: Member \
             must have value greater than or equal to 0",
            "expected the float member's value requoted, got {:?}",
            requoted.message()
        );
        let untouched = in_float_value_rendering(&integral, is_float);
        assert_eq!(
            untouched.message(),
            integral.message(),
            "expected an integral member's clause untouched, got {:?}",
            untouched.message()
        );
    }

    /// RULE: a refusal about a map's KEY is the framework's envelope over a clause that
    /// names the map MEMBER, states no value (a map has no scalar rendering) and lists
    /// what a key has to satisfy, in the framework's own requirement sentences.
    ///
    /// The rows differ along the member path and along the bounds and pattern the key
    /// shape declares, so nothing of one caller's shape is baked into the rendering.
    #[test]
    fn a_map_key_refusal_states_what_a_key_must_satisfy() {
        // (case, member path, minimum, maximum, pattern, the message stated)
        let rows: [(&str, &str, i64, i64, &str, &str); 2] = [
            (
                "a top-level map member",
                "parameters",
                1,
                192,
                "^[.$a-zA-Z0-9_-]+$",
                "1 validation error detected: Value at 'parameters' failed to satisfy \
                 constraint: Map keys must satisfy constraint: [Member must have length less \
                 than or equal to 192, Member must have length greater than or equal to 1, \
                 Member must satisfy regular expression pattern: ^[.$a-zA-Z0-9_-]+$]",
            ),
            (
                "a map nested under a list element",
                "entries.2.member.tags",
                0,
                128,
                "[a-z]+",
                "1 validation error detected: Value at 'entries.2.member.tags' failed to \
                 satisfy constraint: Map keys must satisfy constraint: [Member must have \
                 length less than or equal to 128, Member must have length greater than or \
                 equal to 0, Member must satisfy regular expression pattern: [a-z]+]",
            ),
        ];

        for (case, member_path, min, max, pattern, expected) in rows {
            let stated = map_key_refusal(member_path, min, max, pattern);
            assert_eq!(
                stated, expected,
                "case {case:?}: expected {expected:?}, got {stated:?}"
            );
        }
    }

    /// RULE: the same refusal QUOTES the map the key was held in for the service whose
    /// recordings show that half, and lists the `@pattern` only where the key shape
    /// declares one — everything else (the envelope, the member path, the requirement
    /// sentences) is the same rendering.
    ///
    /// The rows differ along whether the map is quoted, how many entries it holds and
    /// whether the key shape carries a pattern, so neither half is baked into the other.
    #[test]
    fn a_quoted_map_key_refusal_states_the_map_it_refused() {
        /// One row: the member path, the map as quoted (`None` for the valueless clause),
        /// the key shape's `@length` bounds and its `@pattern`, and the message stated.
        struct Row {
            case: &'static str,
            member_path: &'static str,
            value: Option<&'static str>,
            min: i64,
            max: i64,
            pattern: Option<&'static str>,
            expected: &'static str,
        }

        let rows = [
            Row {
                case: "a key shape with a pattern, quoting the map",
                member_path: "roles",
                value: Some("{nonsense=arn:aws:iam::000000000000:role/R}"),
                min: 1,
                max: 128,
                pattern: Some("(un)?authenticated"),
                expected: "1 validation error detected: Value \
                           '{nonsense=arn:aws:iam::000000000000:role/R}' at 'roles' failed to \
                           satisfy constraint: Map keys must satisfy constraint: [Member must \
                           have length less than or equal to 128, Member must have length \
                           greater than or equal to 1, Member must satisfy regular expression \
                           pattern: (un)?authenticated]",
            },
            Row {
                case: "a key shape with no pattern, quoting a two-entry map",
                member_path: "principalTags",
                value: Some("{a=sub, b=aud}"),
                min: 1,
                max: 128,
                pattern: None,
                expected: "1 validation error detected: Value '{a=sub, b=aud}' at \
                           'principalTags' failed to satisfy constraint: Map keys must satisfy \
                           constraint: [Member must have length less than or equal to 128, \
                           Member must have length greater than or equal to 1]",
            },
            Row {
                case: "no map quoted at all",
                member_path: "principalTags",
                value: None,
                min: 1,
                max: 128,
                pattern: None,
                expected: "1 validation error detected: Value at 'principalTags' failed to \
                           satisfy constraint: Map keys must satisfy constraint: [Member must \
                           have length less than or equal to 128, Member must have length \
                           greater than or equal to 1]",
            },
        ];

        for row in rows {
            let stated = map_key_refusal_quoting(
                row.member_path,
                row.value,
                row.min,
                row.max,
                row.pattern,
            );
            assert_eq!(
                stated, row.expected,
                "case {:?}: expected {:?}, got {stated:?}",
                row.case, row.expected
            );
        }
    }

    /// RULE: the requirements a key clause lists are the CALLER's where the key shape's
    /// constraint is one the bounds-and-pattern parameters cannot express — an enumerated
    /// value set — and everything else about the message is the same body
    /// [`map_key_refusal_quoting`] runs.
    ///
    /// The rows differ along the constraint class the key shape declares, how many
    /// requirements it states and whether the map is quoted.
    #[test]
    fn a_map_key_refusal_states_the_requirements_it_was_given() {
        /// One row: the member path, the map as quoted (`None` for the valueless clause),
        /// the requirements the key shape declares, and the message stated.
        struct Row {
            case: &'static str,
            member_path: &'static str,
            value: Option<&'static str>,
            requirements: Vec<String>,
            expected: &'static str,
        }

        let enum_set = requirement(ViolationKind::EnumValue(vec![
            "INDEX_SLOW_LOGS".to_owned(),
            "AUDIT_LOGS".to_owned(),
        ]));
        let length = requirement(ViolationKind::MaxLength(128));
        let rows = [
            Row {
                case: "an enumerated key set, quoting the map",
                member_path: "logPublishingOptions",
                value: Some("{ACCESS_LOGS=Option(enabled=true)}"),
                requirements: vec![enum_set.clone()],
                expected: "1 validation error detected: Value \
                           '{ACCESS_LOGS=Option(enabled=true)}' at 'logPublishingOptions' \
                           failed to satisfy constraint: Map keys must satisfy constraint: \
                           [Member must satisfy enum value set: [INDEX_SLOW_LOGS, AUDIT_LOGS]]",
            },
            Row {
                case: "the same set with no map quoted",
                member_path: "logPublishingOptions",
                value: None,
                requirements: vec![enum_set.clone()],
                expected: "1 validation error detected: Value at 'logPublishingOptions' failed \
                           to satisfy constraint: Map keys must satisfy constraint: [Member \
                           must satisfy enum value set: [INDEX_SLOW_LOGS, AUDIT_LOGS]]",
            },
            Row {
                case: "two requirements, in the order they were given",
                member_path: "tags",
                value: Some("{k=v}"),
                requirements: vec![length.clone(), enum_set.clone()],
                expected: "1 validation error detected: Value '{k=v}' at 'tags' failed to \
                           satisfy constraint: Map keys must satisfy constraint: [Member must \
                           have length less than or equal to 128, Member must satisfy enum \
                           value set: [INDEX_SLOW_LOGS, AUDIT_LOGS]]",
            },
        ];

        for row in rows {
            let stated =
                map_key_requirements_refusal(row.member_path, row.value, &row.requirements);
            assert_eq!(
                stated, row.expected,
                "case {:?}: expected {:?}, got {stated:?}",
                row.case, row.expected
            );
        }
    }

    #[test]
    fn a_constraint_refusal_is_stated_as_aws_states_it() {
        let refusal = Refusal::Invalid(vec![violation(
            "identifiers.1.member.featureGroupName",
            ViolationKind::Pattern(r"^([a-zA-Z0-9]([-_]*[a-zA-Z0-9]){0,63})$".to_owned()),
            "not a valid name!!",
        )]);
        let stated = aws_validation_refusal(&refusal).expect("a constraint refusal is stated");
        assert_eq!(
            stated.code, "ValidationException",
            "expected the JSON protocols' own refusal code, got {:?}",
            stated.code
        );
        assert_eq!(
            stated.message,
            "1 validation error detected: Value 'not a valid name!!' at \
             'identifiers.1.member.featureGroupName' failed to satisfy constraint: Member must \
             satisfy regular expression pattern: ([a-zA-Z0-9]([-_]*[a-zA-Z0-9]){0,63})",
            "expected the AWS envelope with the pattern quoted unanchored, got {:?}",
            stated.message
        );
    }

    /// RULE: a COLLECTION member that broke its `@length` is stated with NO value —
    /// `Value at '<path>' failed to satisfy constraint: <requirement>` — while a SCALAR
    /// member keeps the value it submitted.
    #[test]
    fn a_collection_length_clause_states_no_value_and_a_scalar_one_does() {
        // (case, path, the broken bound, the echo the validating walk rendered, the clause
        // the refusal must state). The echoes are the framework's own: `<n> items` for a
        // list, `<n> entries` for a map, and the caller's own bytes for a string.
        let rows: [(&str, &str, ViolationKind, &str, &str); 8] = [
            (
                "a list one element over its maximum",
                "queries",
                ViolationKind::MaxLength(1),
                "2 items",
                "Value at 'queries' failed to satisfy constraint: Member must have length less \
                 than or equal to 1",
            ),
            (
                "a list far over a much larger maximum",
                "sources",
                ViolationKind::MaxLength(1000),
                "4096 items",
                "Value at 'sources' failed to satisfy constraint: Member must have length less \
                 than or equal to 1000",
            ),
            (
                "an empty list under its minimum",
                "sources",
                ViolationKind::MinLength(1),
                "0 items",
                "Value at 'sources' failed to satisfy constraint: Member must have length \
                 greater than or equal to 1",
            ),
            (
                "a nested list, reported under its element path",
                "entries.1.member.values",
                ViolationKind::MaxLength(5),
                "6 items",
                "Value at 'entries.1.member.values' failed to satisfy constraint: Member must \
                 have length less than or equal to 5",
            ),
            (
                "a map over its maximum",
                "tags",
                ViolationKind::MaxLength(200),
                "201 entries",
                "Value at 'tags' failed to satisfy constraint: Member must have length less than \
                 or equal to 200",
            ),
            (
                "an empty map under its minimum",
                "tags",
                ViolationKind::MinLength(1),
                "0 entries",
                "Value at 'tags' failed to satisfy constraint: Member must have length greater \
                 than or equal to 1",
            ),
            (
                "a string over its maximum keeps the value it submitted",
                "name",
                ViolationKind::MaxLength(3),
                "abcd",
                "Value 'abcd' at 'name' failed to satisfy constraint: Member must have length \
                 less than or equal to 3",
            ),
            (
                "an empty string under its minimum keeps the value it submitted",
                "nextToken",
                ViolationKind::MinLength(1),
                "",
                "Value '' at 'nextToken' failed to satisfy constraint: Member must have length \
                 greater than or equal to 1",
            ),
        ];

        for (case, path, kind, stored, clause) in rows {
            let refusal = Refusal::Invalid(vec![violation(path, kind, stored)]);
            for (entry_point, stated) in [
                ("the default rendering", aws_validation_refusal(&refusal)),
                (
                    "the grouping rendering",
                    aws_validation_refusal_with(&refusal, unanchored_grouping_alternation),
                ),
            ] {
                let stated = stated.expect("a constraint refusal is stated");
                assert_eq!(
                    stated.code, "ValidationException",
                    "case {case:?} under {entry_point}: expected the JSON protocols' refusal \
                     code, got {:?}",
                    stated.code
                );
                let expected = format!("1 validation error detected: {clause}");
                assert_eq!(
                    stated.message, expected,
                    "case {case:?} under {entry_point}: expected {expected:?}, got {:?}",
                    stated.message
                );
            }
        }
    }

    /// RULE, over a refusal carrying SEVERAL clauses: each clause is stated by its own
    /// shape's rule — the collection's without a value, the scalars' with one — inside the
    /// framework's counting envelope.
    #[test]
    fn an_envelope_states_each_clause_by_its_own_shapes_rule() {
        let refusal = Refusal::Invalid(vec![
            violation("queries", ViolationKind::MaxLength(1), "2 items"),
            violation("tags", ViolationKind::MinLength(1), "0 entries"),
            violation("maxResults", ViolationKind::MinValue("1".to_owned()), "0"),
        ]);
        let stated = aws_validation_refusal(&refusal).expect("a constraint refusal is stated");
        let expected = "3 validation errors detected: Value at 'queries' failed to satisfy \
                        constraint: Member must have length less than or equal to 1; Value at \
                        'tags' failed to satisfy constraint: Member must have length greater \
                        than or equal to 1; Value '0' at 'maxResults' failed to satisfy \
                        constraint: Member must have value greater than or equal to 1";
        assert_eq!(
            stated.message, expected,
            "expected {expected:?}, got {:?}",
            stated.message
        );
    }

    /// RULE: only a `@length` violation of a collection loses its value — a collection's
    /// OTHER constraint classes, and a member whose value the walk could not render,
    /// keep the framework's clause.
    #[test]
    fn nothing_but_a_collection_length_clause_loses_its_value() {
        // (case, the violation, the clause the refusal must state)
        let rows: [(&str, Violation, &str); 3] = [
            (
                "a list with a repeated element",
                violation("values", ViolationKind::UniqueItems, "duplicate"),
                "Value 'duplicate' at 'values' failed to satisfy constraint: Member must have \
                 unique values",
            ),
            (
                "an absent required member",
                Violation {
                    path: "queries".to_owned(),
                    kind: ViolationKind::MissingRequiredMember,
                    value: None,
                    sensitive: false,
                },
                "Value null at 'queries' failed to satisfy constraint: Member must not be null",
            ),
            (
                "an absent member carrying a length bound",
                Violation {
                    path: "sources".to_owned(),
                    kind: ViolationKind::MinLength(1),
                    value: None,
                    sensitive: false,
                },
                "Value null at 'sources' failed to satisfy constraint: Member must have length \
                 greater than or equal to 1",
            ),
        ];

        for (case, violation, clause) in rows {
            let stated = aws_validation_refusal(&Refusal::Invalid(vec![violation]))
                .expect("a constraint refusal is stated");
            let expected = format!("1 validation error detected: {clause}");
            assert_eq!(
                stated.message, expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                stated.message
            );
        }
    }

    /// RULE: a member the caller WITHHOLDS the value of is stated with no value half at
    /// all, whichever half the framework wrote and whatever the constraint was — and the
    /// members it does not withhold keep theirs, in the same envelope.
    #[test]
    fn a_withheld_members_clause_states_no_value() {
        // (case, the violation, the clause the refusal must state for the withheld member)
        let rows: [(&str, Violation, &str); 4] = [
            (
                "a withheld member that was absent",
                Violation {
                    path: "secret".to_owned(),
                    kind: ViolationKind::MissingRequiredMember,
                    value: None,
                    sensitive: false,
                },
                "Value at 'secret' failed to satisfy constraint: Member must not be null",
            ),
            (
                "a withheld member shorter than its minimum",
                violation("secret", ViolationKind::MinLength(4), "ab"),
                "Value at 'secret' failed to satisfy constraint: Member must have length \
                 greater than or equal to 4",
            ),
            (
                "a withheld member longer than its maximum",
                violation("secret", ViolationKind::MaxLength(8), "abcdefghij"),
                "Value at 'secret' failed to satisfy constraint: Member must have length less \
                 than or equal to 8",
            ),
            (
                "a withheld member outside its pattern",
                violation(
                    "secret",
                    ViolationKind::Pattern("^[a-z]+$".to_owned()),
                    "A B",
                ),
                "Value at 'secret' failed to satisfy constraint: Member must satisfy regular \
                 expression pattern: [a-z]+",
            ),
        ];

        for (case, offending, clause) in rows {
            let refusal = Refusal::Invalid(vec![offending]);
            let stated = aws_validation_refusal_withholding(&refusal, |path| path == "secret")
                .expect("a constraint refusal is stated");
            let expected = format!("1 validation error detected: {clause}");
            assert_eq!(
                stated.message, expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                stated.message
            );
            // The same refusal, with nothing withheld, keeps the framework's value half:
            // the suppression is the caller's decision and nothing else about the clause.
            let kept = aws_validation_refusal(&refusal).expect("a constraint refusal is stated");
            assert_ne!(
                kept.message, expected,
                "case {case:?}: withholding nothing must keep the value half, got {:?}",
                kept.message
            );
        }

        // One envelope over both kinds of member: only the withheld one loses its value.
        let refusal = Refusal::Invalid(vec![
            violation("secret", ViolationKind::MinLength(4), "ab"),
            violation("name", ViolationKind::MinLength(2), "a"),
        ]);
        let stated = aws_validation_refusal_withholding(&refusal, |path| path == "secret")
            .expect("a constraint refusal is stated");
        let expected = "2 validation errors detected: Value at 'secret' failed to satisfy \
                        constraint: Member must have length greater than or equal to 4; Value \
                        'a' at 'name' failed to satisfy constraint: Member must have length \
                        greater than or equal to 2";
        assert_eq!(
            stated.message, expected,
            "expected {expected:?}, got {:?}",
            stated.message
        );
    }

    /// RULE: the value half is dropped for a whole CONSTRAINT CLASS when the caller decides
    /// withholding from the violation — here the `@pattern` class — and every other class of
    /// the same refusal, and the same member, keeps the framework's value half.
    #[test]
    fn a_withheld_class_states_no_value_for_that_class_alone() {
        // The decision a service whose recordings state a pattern requirement over the
        // expression alone passes: the CLASS, not the member.
        let withholds = |violation: &Violation| matches!(violation.kind, ViolationKind::Pattern(_));
        // (case, the violations, the message stated)
        let rows: [(&str, Vec<Violation>, &str); 3] = [
            (
                "one member outside its pattern",
                vec![violation(
                    "name",
                    ViolationKind::Pattern("^[a-z]+$".to_owned()),
                    "A B",
                )],
                "1 validation error detected: Value at 'name' failed to satisfy constraint: \
                 Member must satisfy regular expression pattern: [a-z]+",
            ),
            (
                "another class keeps its value half",
                vec![violation("name", ViolationKind::MinLength(4), "ab")],
                "1 validation error detected: Value 'ab' at 'name' failed to satisfy \
                 constraint: Member must have length greater than or equal to 4",
            ),
            (
                "one member breaking both classes",
                vec![
                    violation("name", ViolationKind::MinLength(4), "A B"),
                    violation("name", ViolationKind::Pattern("^[a-z]+$".to_owned()), "A B"),
                ],
                "2 validation errors detected: Value 'A B' at 'name' failed to satisfy \
                 constraint: Member must have length greater than or equal to 4; Value at \
                 'name' failed to satisfy constraint: Member must satisfy regular expression \
                 pattern: [a-z]+",
            ),
        ];

        for (case, violations, expected) in rows {
            let refusal = Refusal::Invalid(violations);
            let stated = aws_validation_refusal_withholding_violations(&refusal, withholds)
                .expect("a constraint refusal is stated");
            assert_eq!(
                stated.code, "ValidationException",
                "case {case:?}: unexpected code, got {:?}",
                stated.code
            );
            assert_eq!(
                stated.message, expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                stated.message
            );
        }

        for (case, refusal) in [
            ("an unauthenticated caller", Refusal::Unauthenticated),
            ("no violation at all", Refusal::Invalid(Vec::new())),
        ] {
            assert!(
                aws_validation_refusal_withholding_violations(&refusal, withholds).is_none(),
                "case {case:?}: expected the framework's own wording"
            );
        }
    }

    /// RULE: the entry point that takes BOTH choices applies both — the caller's
    /// `@pattern` rendering and its withholding — over the one envelope every other entry
    /// point here renders.
    ///
    /// The rows differ in which of the two choices each clause exercises: a withheld
    /// member outside its pattern loses its value half AND keeps the expression as the
    /// caller quotes it, while a member neither choice touches keeps both halves.
    #[test]
    fn both_choices_apply_over_the_one_envelope() {
        // The rendering a service whose recordings quote the model's expression verbatim
        // passes, anchors included.
        let as_written = |pattern: &str| pattern.to_owned();
        // (case, the violations, the message stated)
        let rows: [(&str, Vec<Violation>, &str); 3] = [
            (
                "a withheld member outside its pattern",
                vec![violation(
                    "secret",
                    ViolationKind::Pattern("^[a-z]+$".to_owned()),
                    "A B",
                )],
                "1 validation error detected: Value at 'secret' failed to satisfy constraint: \
                 Member must satisfy regular expression pattern: ^[a-z]+$",
            ),
            (
                "a member neither choice touches",
                vec![violation(
                    "name",
                    ViolationKind::Pattern("^[a-z]+$".to_owned()),
                    "A B",
                )],
                "1 validation error detected: Value 'A B' at 'name' failed to satisfy \
                 constraint: Member must satisfy regular expression pattern: ^[a-z]+$",
            ),
            (
                "both in one envelope",
                vec![
                    violation("secret", ViolationKind::MinLength(4), "ab"),
                    violation(
                        "name",
                        ViolationKind::Pattern("^[a-z]+$".to_owned()),
                        "A B",
                    ),
                ],
                "2 validation errors detected: Value at 'secret' failed to satisfy constraint: \
                 Member must have length greater than or equal to 4; Value 'A B' at 'name' \
                 failed to satisfy constraint: Member must satisfy regular expression pattern: \
                 ^[a-z]+$",
            ),
        ];

        for (case, violations, expected) in rows {
            let stated = aws_validation_refusal_withholding_with(
                &Refusal::Invalid(violations),
                as_written,
                |path| path == "secret",
            )
            .expect("a constraint refusal is stated");
            assert_eq!(
                stated.code, "ValidationException",
                "case {case:?}: expected the JSON protocols' own refusal code, got {:?}",
                stated.code
            );
            assert_eq!(
                stated.message, expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                stated.message
            );
        }
    }

    #[test]
    fn a_pattern_requirement_is_quoted_by_the_one_renderer_this_crate_owns() {
        // Each row is (case, the model's pattern, the expression the clause must state) —
        // exactly what `crate::validation::unanchored` renders, which is the single
        // implementation of that rule and is table-tested in its own module. These rows
        // only pin that this restatement goes THROUGH it: an outer pair is dropped, and
        // anything else (a lone anchor, an alternation's own interior anchors) stays
        // verbatim.
        let rows: [(&str, &str, &str); 5] = [
            (
                "a wrapping pair",
                r"^[a-zA-Z0-9]([-_]*[a-zA-Z0-9]){0,63}$",
                r"[a-zA-Z0-9]([-_]*[a-zA-Z0-9]){0,63}",
            ),
            ("a leading anchor only", r"^arn:aws.*", r"^arn:aws.*"),
            ("a trailing anchor only", r"[0-9]{12}$", r"[0-9]{12}$"),
            (
                "an alternation whose branches carry the anchors",
                r"(^a$)|(^b$)",
                r"(^a$)|(^b$)",
            ),
            // The row this one previously stated as unchanged: without the parentheses the
            // branches SHARE the outer pair, which the rule drops positionally.
            (
                "an alternation sharing the outer pair",
                r"^a$|^b$",
                r"a$|^b",
            ),
        ];
        for (case, pattern, expected) in rows {
            let refusal = Refusal::Invalid(vec![violation(
                "name",
                ViolationKind::Pattern(pattern.to_owned()),
                "x",
            )]);
            let stated = aws_validation_refusal(&refusal).expect("a constraint refusal is stated");
            let reported = stated
                .message
                .rsplit("regular expression pattern: ")
                .next()
                .unwrap_or_default();
            assert_eq!(
                reported, expected,
                "case {case:?}: expected {pattern:?} to be stated as {expected:?}, got \
                 {reported:?}"
            );
        }
    }

    #[test]
    fn a_requirement_that_is_not_a_pattern_keeps_the_frameworks_wording() {
        let refusal = Refusal::Invalid(vec![violation(
            "maxResults",
            ViolationKind::MaxValue("100".to_owned()),
            "1000",
        )]);
        let stated = aws_validation_refusal(&refusal).expect("a constraint refusal is stated");
        assert!(
            stated.message.contains("at 'maxResults'")
                && stated
                    .message
                    .contains("Member must have value less than or equal to 100")
                && !stated.message.contains("regular expression pattern"),
            "expected the framework's own numeric-bound requirement, got {:?}",
            stated.message
        );
    }

    /// RULE: the chosen rendering is the ONLY thing that differs between the two entry
    /// points — same code, same envelope, same clause around a differently quoted
    /// expression — and a caller that passes the grouping rendering gets it.
    #[test]
    fn the_chosen_pattern_rendering_is_the_only_difference_between_the_two_entry_points() {
        // (case, the model's pattern, what the default states, what the grouping rendering
        // states). The expressions are `crate::validation`'s own table rows: a wrapping
        // pair, and a pair the branches of a top-level alternation share.
        let rows: [(&str, &str, &str, &str); 3] = [
            (
                "a wrapping pair, where the two renderings agree",
                r"^[0-9]{12}$",
                r"[0-9]{12}",
                r"[0-9]{12}",
            ),
            (
                "a top-level alternation of two groups",
                r"^(arn:[^:]+:x)|([a-f0-9]{8})$",
                r"(arn:[^:]+:x)|([a-f0-9]{8})",
                r"((arn:[^:]+:x)|([a-f0-9]{8}))",
            ),
            (
                "a bare top-level alternation",
                r"^a|b$",
                r"a|b",
                r"(a|b)",
            ),
        ];
        for (case, pattern, plain, grouped) in rows {
            let refusal = Refusal::Invalid(vec![violation(
                "name",
                ViolationKind::Pattern(pattern.to_owned()),
                "x",
            )]);
            for (rendering, expected, stated) in [
                ("the default", plain, aws_validation_refusal(&refusal)),
                (
                    "the grouping rendering",
                    grouped,
                    aws_validation_refusal_with(&refusal, unanchored_grouping_alternation),
                ),
            ] {
                let stated = stated.expect("a constraint refusal is stated");
                assert_eq!(
                    stated.code, "ValidationException",
                    "case {case:?} under {rendering}: expected the JSON protocols' refusal \
                     code, got {:?}",
                    stated.code
                );
                let one = format!(
                    "1 validation error detected: Value 'x' at 'name' failed to satisfy \
                     constraint: Member must satisfy regular expression pattern: {expected}"
                );
                assert_eq!(
                    stated.message, one,
                    "case {case:?} under {rendering}: expected {one:?}, got {:?}",
                    stated.message
                );
            }
        }
    }

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
        assert!(
            aws_validation_refusal_with(&Refusal::Unauthenticated, unanchored_grouping_alternation)
                .is_none(),
            "expected the chosen rendering not to change which refusals are stated"
        );
    }

    /// RULE: the two string findings state the value the caller submitted as it was
    /// submitted — the expression and the string for a pattern, the string and both
    /// lengths for a maximum — with nothing of the member or the envelope around them.
    ///
    /// The rows differ along what the sentences generalise over: the form of the
    /// expression, and the value's length, charset and content (including one carrying the
    /// quotes and braces the sentence itself uses).
    #[test]
    fn the_string_findings_state_the_submitted_value_and_the_schemas_own_bound() {
        // (case, the expression, the value, the finding expected)
        let patterns: [(&str, &str, &str, &str); 3] = [
            (
                "an anchored expression",
                r"^[\.\-_A-Za-z0-9]+$",
                "bad@name",
                r#"ECMA 262 regex "^[\.\-_A-Za-z0-9]+$" does not match input string "bad@name""#,
            ),
            (
                "an alternation",
                r"^smk://host:[0-9]{1,5}|arn:.+$",
                "https://host/queue",
                r#"ECMA 262 regex "^smk://host:[0-9]{1,5}|arn:.+$" does not match input string "https://host/queue""#,
            ),
            (
                "a value carrying the sentence's own punctuation",
                r"^[a-z]+$",
                r#"{"a": "b"}"#,
                r#"ECMA 262 regex "^[a-z]+$" does not match input string "{"a": "b"}""#,
            ),
        ];
        for (case, pattern, value, expected) in patterns {
            let actual = pattern_mismatch(pattern, value);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }

        // (case, the value, the length the constraint counted, the maximum)
        let lengths: [(&str, &str, usize, i64); 3] = [
            ("one character past the maximum", "xxx", 3, 2),
            // `@length` counts CHARACTERS, so the length is the one that was checked and
            // not the byte count of the value.
            ("a unicode value", "ééé", 3, 2),
            ("a maximum of zero", "x", 1, 0),
        ];
        for (case, value, length, max) in lengths {
            let expected =
                format!(r#"string "{value}" is too long (length: {length}, maximum allowed: {max})"#);
            let actual = string_too_long(value, length, max);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }

        // The same rows against the MINIMUM bound: (case, the value, the length the
        // constraint counted, the minimum).
        let minimums: [(&str, &str, usize, i64); 3] = [
            ("one character short of the minimum", "x", 1, 2),
            ("the empty string", "", 0, 1),
            ("a unicode value", "é", 1, 2),
        ];
        for (case, value, length, min) in minimums {
            let expected = format!(
                r#"string "{value}" is too short (length: {length}, required minimum: {min})"#
            );
            let actual = string_too_short(value, length, min);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }

        // (case, the type the instance has, the types the schema admits, the finding)
        let types: [(&str, &str, &[&str], &str); 3] = [
            (
                "an array where an object is admitted",
                "array",
                &["object"],
                "instance type (array) does not match any allowed primitive type (allowed: \
                 [\"object\"])",
            ),
            (
                "a number where a string is admitted",
                "integer",
                &["string"],
                "instance type (integer) does not match any allowed primitive type (allowed: \
                 [\"string\"])",
            ),
            (
                "two admitted types",
                "null",
                &["string", "array"],
                "instance type (null) does not match any allowed primitive type (allowed: \
                 [\"string\",\"array\"])",
            ),
        ];
        for (case, found, allowed, expected) in types {
            let actual = instance_type_mismatch(found, allowed);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }

        // (case, the elements the instance carries, the minimum the schema declares)
        let arrays: [(&str, usize, i64); 2] = [
            ("an empty array where one element is required", 0, 1),
            ("one element short of the minimum", 2, 3),
        ];
        for (case, found, min) in arrays {
            let expected = format!(
                "array is too short: must have at least {min} elements but instance has {found} \
                 elements"
            );
            let actual = array_too_short(found, min);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: the pattern report lists one finding per member outside its `@pattern`,
    /// inside ONE pair of brackets and in the order the walk found them, and declines a
    /// refusal holding any other constraint class.
    ///
    /// The rows span what the report generalises over: how many members broke a pattern,
    /// the expressions and values they carried, a withheld (`@sensitive`) value, the other
    /// constraint classes, a mixed refusal, and the empty one.
    #[test]
    fn the_pattern_report_brackets_one_finding_per_member_outside_its_pattern() {
        let one = violation(
            "name",
            ViolationKind::Pattern(r"^[A-Za-z0-9]+$".to_owned()),
            "bad name",
        );
        let another = violation(
            "clientToken",
            ViolationKind::Pattern(r"^[A-Za-z0-9_.-]{0,63}$".to_owned()),
            "bad token",
        );
        let withheld = Violation {
            value: None,
            sensitive: true,
            ..one.clone()
        };

        // (case, the violations, the report expected — `None` keeps the framework's)
        let rows: [(&str, Vec<Violation>, Option<String>); 7] = [
            (
                "one member outside its pattern",
                vec![one.clone()],
                Some(
                    r#"[ECMA 262 regex "^[A-Za-z0-9]+$" does not match input string "bad name"]"#
                        .to_owned(),
                ),
            ),
            (
                "two members, in the order the walk found them",
                vec![one.clone(), another.clone()],
                Some(
                    r#"[ECMA 262 regex "^[A-Za-z0-9]+$" does not match input string "bad name", ECMA 262 regex "^[A-Za-z0-9_.-]{0,63}$" does not match input string "bad token"]"#
                        .to_owned(),
                ),
            ),
            (
                "the same two the other way round",
                vec![another.clone(), one.clone()],
                Some(
                    r#"[ECMA 262 regex "^[A-Za-z0-9_.-]{0,63}$" does not match input string "bad token", ECMA 262 regex "^[A-Za-z0-9]+$" does not match input string "bad name"]"#
                        .to_owned(),
                ),
            ),
            (
                "a withheld value states the expression alone",
                vec![withheld],
                Some(
                    r#"[ECMA 262 regex "^[A-Za-z0-9]+$" does not match input string ""]"#
                        .to_owned(),
                ),
            ),
            (
                "another constraint class on its own",
                vec![violation("name", ViolationKind::MinLength(2), "a")],
                None,
            ),
            (
                "a pattern mixed with another class",
                vec![violation("name", ViolationKind::MinLength(2), "a"), one],
                None,
            ),
            ("no violation at all", Vec::new(), None),
        ];
        for (case, violations, expected) in rows {
            let actual = pattern_constraint_report(&violations);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: the enumerated-set finding states the value the caller submitted and every
    /// value the schema admits, in the order they were declared, with nothing of the member
    /// or the envelope around them.
    ///
    /// The rows differ along what the sentence generalises over: how many values the set
    /// holds, their order, and the form of the value that missed it (another spelling, a
    /// different case, the empty string).
    #[test]
    fn the_enumerated_set_finding_states_the_submitted_value_and_every_admitted_one() {
        // (case, the value, the values the schema admits, the finding expected)
        let rows: [(&str, &str, &[&str], &str); 5] = [
            (
                "a value of no admitted spelling",
                "NOTAVALIDTYPE",
                &["IPV4", "DUALSTACK"],
                r#"instance value ("NOTAVALIDTYPE") not found in enum (possible values: ["IPV4","DUALSTACK"])"#,
            ),
            (
                "an admitted value in another case",
                "ipv4",
                &["IPV4", "DUALSTACK"],
                r#"instance value ("ipv4") not found in enum (possible values: ["IPV4","DUALSTACK"])"#,
            ),
            (
                "a set of three, in its declared order",
                "MAYBE",
                &["ATLEAST", "AND", "OR"],
                r#"instance value ("MAYBE") not found in enum (possible values: ["ATLEAST","AND","OR"])"#,
            ),
            (
                "a set of one",
                "ARCHIVE",
                &["EXPIRE"],
                r#"instance value ("ARCHIVE") not found in enum (possible values: ["EXPIRE"])"#,
            ),
            (
                "the empty string",
                "",
                &["IPV4", "DUALSTACK"],
                r#"instance value ("") not found in enum (possible values: ["IPV4","DUALSTACK"])"#,
            ),
        ];
        for (case, value, permitted, expected) in rows {
            let actual = enum_value_not_found(value, permitted);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: a value set a service publishes an order of its own for is stated in that
    /// order wherever it is reported — restated on the violation and restated inside an
    /// already-worded sentence alike — while every other set keeps the order it arrived in.
    ///
    /// The rows differ along what the statement generalises over: the order the declared
    /// set is presented in, whether the service's half lists every declared value, which
    /// of several published enums the set belongs to, and a set with no row at all.
    #[test]
    fn a_published_value_set_is_stated_in_the_services_own_order() {
        // Two published sets: one listing the declared values in the service's own order,
        // one listing FEWER values than the model declares.
        const REORDERED_DECLARED: [&str; 3] = ["custom", "managed", "shared"];
        const REORDERED_STATED: [&str; 3] = ["managed", "shared", "custom"];
        const LAGGING_DECLARED: [&str; 3] = ["ssml", "text", "markdown"];
        const LAGGING_STATED: [&str; 2] = ["text", "ssml"];
        const PUBLISHED: &StatedEnumValueSets<'static> = &[
            (&REORDERED_DECLARED, &REORDERED_STATED),
            (&LAGGING_DECLARED, &LAGGING_STATED),
        ];

        /// One row: the case, the set as it was presented, and the values stated for it.
        type Row = (&'static str, &'static [&'static str], Option<&'static [&'static str]>);

        let rows: [Row; 6] = [
            (
                "a published set, in declared order",
                &REORDERED_DECLARED,
                Some(&REORDERED_STATED),
            ),
            (
                "the same set, presented in another order",
                &["shared", "custom", "managed"],
                Some(&REORDERED_STATED),
            ),
            (
                "a published set the service lists fewer values of",
                &LAGGING_DECLARED,
                Some(&LAGGING_STATED),
            ),
            ("a set with no row", &["IPV4", "DUALSTACK"], None),
            (
                "a published set missing one value is another set",
                &["custom", "managed"],
                None,
            ),
            ("no values at all", &[], None),
        ];
        for (case, presented, stated) in rows {
            let declared: Vec<String> = presented.iter().map(|v| (*v).to_owned()).collect();
            let found = stated_enum_value_set(&declared, PUBLISHED);
            assert_eq!(
                found, stated,
                "case {case:?}: expected {stated:?}, got {found:?}"
            );

            // On the violation: only the values move, and the clause is the framework's.
            let reported = violation("policyType", ViolationKind::EnumValue(declared), "nope");
            let restated = in_stated_enum_order(&reported, PUBLISHED);
            let expected = match stated {
                Some(stated) => ViolationKind::EnumValue(
                    stated.iter().map(|v| (*v).to_owned()).collect(),
                ),
                None => reported.kind.clone(),
            };
            assert_eq!(
                restated.kind, expected,
                "case {case:?}: wrong value set on the violation, got {:?}",
                restated.kind
            );
            assert_eq!(
                (restated.path.as_str(), restated.value.as_deref()),
                ("policyType", Some("nope")),
                "case {case:?}: nothing but the value set may move"
            );
        }

        // Two sets in one refusal, each stated on its own terms, and every other
        // violation kind left alone.
        let reported = [
            violation(
                "policyType",
                ViolationKind::EnumValue(
                    REORDERED_DECLARED.iter().map(|v| (*v).to_owned()).collect(),
                ),
                "nope",
            ),
            violation(
                "addressType",
                ViolationKind::EnumValue(vec!["IPV4".to_owned(), "DUALSTACK".to_owned()]),
                "other",
            ),
            violation("name", ViolationKind::MinLength(1), ""),
        ];
        let stated: Vec<String> = reported
            .iter()
            .map(|reported| in_stated_enum_order(reported, PUBLISHED).message())
            .collect();
        let expected: Vec<String> = [
            violation(
                "policyType",
                ViolationKind::EnumValue(
                    REORDERED_STATED.iter().map(|v| (*v).to_owned()).collect(),
                ),
                "nope",
            ),
            violation(
                "addressType",
                ViolationKind::EnumValue(vec!["IPV4".to_owned(), "DUALSTACK".to_owned()]),
                "other",
            ),
            violation("name", ViolationKind::MinLength(1), ""),
        ]
        .iter()
        .map(Violation::message)
        .collect();
        assert_eq!(
            stated, expected,
            "each clause must state its own enum's set, got {stated:?}"
        );
    }

    /// RULE: a front door that holds an enumerated type's admissible values in a hash
    /// table keyed on each value's own text states them in the order that table is walked
    /// — ascending bucket, a shared bucket read from the head each value was pushed onto —
    /// which belongs to the TYPE and so does not depend on the order the values reached
    /// the clause in, nor on the member, operation or depth that carried the bad value.
    #[test]
    fn a_hashed_value_set_is_stated_in_the_order_its_table_is_walked() {
        // (case, the values as declared, the order the table is walked in). The first row
        // is a set a refusal is known to state: `ProcessorType` of
        // `com.amazonaws.firehose` (`smithy_query(query="shape", shape="ProcessorType")`),
        // whose six values fall in six distinct buckets.
        let rows: &[(&str, &[&str], &[&str])] = &[
            (
                "six values in six buckets",
                &[
                    "RecordDeAggregation",
                    "Decompression",
                    "CloudWatchLogProcessing",
                    "Lambda",
                    "MetadataExtraction",
                    "AppendDelimiterToRecord",
                ],
                &[
                    "Decompression",
                    "Lambda",
                    "RecordDeAggregation",
                    "MetadataExtraction",
                    "AppendDelimiterToRecord",
                    "CloudWatchLogProcessing",
                ],
            ),
            // Two values of ONE bucket: the chain is read from its head, so the later of
            // the two is stated first. This row is a set a recorded refusal shows a
            // collision in: `com.amazonaws.acm-pca#ValidityPeriodType`, where `ABSOLUTE`
            // and `DAYS` share a bucket and the later-declared `DAYS` is stated first.
            (
                "a declared set with a collision",
                &["END_DATE", "ABSOLUTE", "DAYS", "MONTHS", "YEARS"],
                &["END_DATE", "MONTHS", "YEARS", "DAYS", "ABSOLUTE"],
            ),
            // Two values of ONE bucket: the chain is read from its head, so the later of
            // the two is stated first. `RoleArn` and `Delimiter` are both values of
            // `com.amazonaws.firehose#ProcessorParameterName` and their hashes agree in
            // the low bits.
            (
                "two values sharing a bucket",
                &["RoleArn", "Delimiter"],
                &["Delimiter", "RoleArn"],
            ),
            (
                "the same two, declared the other way round",
                &["Delimiter", "RoleArn"],
                &["RoleArn", "Delimiter"],
            ),
            ("one value", &["GZIP"], &["GZIP"]),
            ("no values at all", &[], &[]),
        ];

        assert_eq!(
            enum_value_bucket("RoleArn"),
            enum_value_bucket("Delimiter"),
            "the collision rows need two values of ONE bucket, got {} and {}",
            enum_value_bucket("RoleArn"),
            enum_value_bucket("Delimiter")
        );

        for (case, declared, expected) in rows {
            let actual = hashed_enum_order(declared);
            assert_eq!(
                actual,
                expected.to_vec(),
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );

            // A set is RE-ORDERED, never changed.
            let mut sorted_actual = actual.clone();
            sorted_actual.sort_unstable();
            let mut sorted_declared = declared.to_vec();
            sorted_declared.sort_unstable();
            assert_eq!(
                sorted_actual, sorted_declared,
                "case {case:?}: the stated set is the declared one re-ordered, got {actual:?}"
            );

            // On the violation: only the values move, and the clause is the framework's.
            let values: Vec<String> = declared.iter().map(|v| (*v).to_owned()).collect();
            let reported = violation("processorType", ViolationKind::EnumValue(values), "nope");
            let restated = in_hashed_enum_order(&reported);
            assert_eq!(
                restated.kind,
                ViolationKind::EnumValue(
                    expected.iter().map(|v| (*v).to_owned()).collect()
                ),
                "case {case:?}: wrong value set on the violation, got {:?}",
                restated.kind
            );
            assert_eq!(
                (restated.path.as_str(), restated.value.as_deref()),
                ("processorType", Some("nope")),
                "case {case:?}: nothing but the value set may move"
            );
        }

        // Every other violation kind is returned unchanged.
        let other = violation("name", ViolationKind::MinLength(1), "");
        assert_eq!(
            in_hashed_enum_order(&other).message(),
            other.message(),
            "a violation that is not an enum value set must be left alone"
        );
    }

    /// RULE: a MAP member over its `@length` maximum is stated as the schema layer's
    /// property-count finding, built from the entry count the request carried and the
    /// maximum the model declares — and nothing else is read as one.
    #[test]
    fn a_map_over_its_length_maximum_is_stated_as_a_property_count_finding() {
        // (case, the stored echo the walk rendered, the `@length` maximum, the whole
        // message expected, or `None` to keep the per-member envelope). The echoes are
        // the framework's own renderings: `<n> entries` for a map, `<n> items` for a
        // list, and the caller's own bytes for a string.
        let rows: [(&str, &str, i64, Option<&str>); 7] = [
            (
                "one entry past the maximum",
                "201 entries",
                200,
                Some("[object has too many properties (found 201 but schema requires at most 200)]"),
            ),
            (
                "far past a much smaller maximum",
                "4096 entries",
                50,
                Some("[object has too many properties (found 4096 but schema requires at most 50)]"),
            ),
            (
                "one entry past a maximum of zero",
                "1 entries",
                0,
                Some("[object has too many properties (found 1 but schema requires at most 0)]"),
            ),
            ("a list, not an object", "7 items", 5, None),
            ("a string the caller sent", "entries", 3, None),
            ("a string that only looks like a count", "lots entries", 3, None),
            // The accepted collision `map_entry_count` documents: a string whose value
            // IS the map rendering cannot be told apart from a map here, so it is stated
            // as a property-count finding. No emulated member's `@length` maximum is low
            // enough for a request to reach it.
            (
                "a string whose own value is the map rendering",
                "7 entries",
                3,
                Some("[object has too many properties (found 7 but schema requires at most 3)]"),
            ),
        ];

        for (case, stored, max, expected) in rows {
            let refusal = Refusal::Invalid(vec![violation(
                "tags",
                ViolationKind::MaxLength(max),
                stored,
            )]);
            let stated = property_count_refusal(&refusal);
            match expected {
                Some(message) => {
                    let stated = stated
                        .unwrap_or_else(|| panic!("case {case:?}: expected {message:?}, got none"));
                    assert_eq!(
                        stated.code, "ValidationException",
                        "case {case:?}: expected the JSON protocols' refusal code, got {:?}",
                        stated.code
                    );
                    assert_eq!(
                        stated.message, message,
                        "case {case:?}: expected {message:?}, got {:?}",
                        stated.message
                    );
                }
                None => assert!(
                    stated.is_none(),
                    "case {case:?}: expected the per-member envelope to stand, got {stated:?}"
                ),
            }
        }
    }

    /// RULE: only a length MAXIMUM on a map is that finding — a map under a `@length`
    /// MINIMUM, and a refusal that is no constraint failure at all, are not.
    #[test]
    fn nothing_but_a_map_over_its_maximum_is_a_property_count_finding() {
        assert!(
            property_count_refusal(&Refusal::Invalid(vec![violation(
                "tags",
                ViolationKind::MinLength(1),
                "0 entries",
            )]))
            .is_none(),
            "expected a map UNDER its minimum to keep the per-member envelope"
        );
        assert!(
            property_count_refusal(&Refusal::Unauthenticated).is_none(),
            "expected an unauthenticated caller to keep the framework's 403 wording"
        );
        assert!(
            property_count_refusal(&Refusal::Invalid(Vec::new())).is_none(),
            "expected no violations to state nothing"
        );
    }

    // A collection member carrying its element shape's own constraints, written the way
    // codegen writes one: the element's `@length`/`@pattern` live in an
    // `ElementConstraintsTrait` wrapper on the COLLECTION member's schema, which also
    // carries the collection's own `@length` over the element count.
    static ELEMENT_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
        let mut traits = TraitMap::new();
        traits.insert(Box::new(LengthTrait::new(Some(1), Some(128))));
        traits.insert(Box::new(PatternTrait::new(r"^[0-9]+$")));
        traits
    });
    static MEMBER_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
        let mut traits = TraitMap::new();
        traits.insert(Box::new(LengthTrait::new(Some(1), Some(1000))));
        traits.insert(Box::new(ElementConstraintsTrait::new(&ELEMENT_TRAITS)));
        traits
    });
    static COLLECTION_MEMBER: Schema = Schema::new_member(
        ShapeId::from_static("test#Input$Numbers", "test", "Numbers"),
        ShapeType::List,
        "Numbers",
        0,
    )
    .with_traits(&MEMBER_TRAITS);
    static SCALAR_MEMBER: Schema = Schema::new_member(
        ShapeId::from_static("test#Input$Name", "test", "Name"),
        ShapeType::String,
        "Name",
        1,
    );
    static INPUT: Schema = Schema::new_struct(
        ShapeId::from_static("test#Input", "test", "Input"),
        ShapeType::Structure,
        &[&COLLECTION_MEMBER, &SCALAR_MEMBER],
    );

    /// The element shape's whole set, as the clause nests it.
    const ELEMENT_SET: &str = "Member must satisfy constraint: [Member must have length less \
                               than or equal to 128, Member must have length greater than or \
                               equal to 1, Member must satisfy regular expression pattern: \
                               ^[0-9]+$]";

    /// RULE: a constraint broken by a collection's ELEMENTS is stated once at the
    /// collection member — no element position, no `member` step and no value — with the
    /// element shape's whole declared set nested inside `Member must satisfy constraint:
    /// [ … ]`, whichever of those constraints the request actually broke and however many
    /// elements broke them.
    #[test]
    fn an_element_constraint_is_stated_at_the_collection_member() {
        // (case, the violations one request produced, the message expected)
        let rows: Vec<(&str, Vec<Violation>, String)> = vec![
            (
                "one element outside the element shape's pattern",
                vec![violation(
                    "numbers.1.member",
                    ViolationKind::Pattern(r"^[0-9]+$".to_owned()),
                    "not-a-number",
                )],
                format!(
                    "1 validation error detected: Value at 'numbers' failed to satisfy \
                     constraint: {ELEMENT_SET}"
                ),
            ),
            (
                "one element breaking a bound the set also declares",
                vec![violation(
                    "numbers.3.member",
                    ViolationKind::MinLength(1),
                    "",
                )],
                format!(
                    "1 validation error detected: Value at 'numbers' failed to satisfy \
                     constraint: {ELEMENT_SET}"
                ),
            ),
            (
                "two elements, each breaking a different one",
                vec![
                    violation(
                        "numbers.1.member",
                        ViolationKind::Pattern(r"^[0-9]+$".to_owned()),
                        "x",
                    ),
                    violation("numbers.2.member", ViolationKind::MaxLength(128), "9999"),
                ],
                format!(
                    "1 validation error detected: Value at 'numbers' failed to satisfy \
                     constraint: {ELEMENT_SET}"
                ),
            ),
            (
                // The collection's OWN `@length` is not an element constraint: it keeps
                // its own clause, with the value half a collection has no rendering for
                // dropped.
                "the collection's own length, beside an element's",
                vec![
                    violation("numbers", ViolationKind::MaxLength(1000), "1001 items"),
                    violation(
                        "numbers.9.member",
                        ViolationKind::Pattern(r"^[0-9]+$".to_owned()),
                        "x",
                    ),
                ],
                format!(
                    "2 validation errors detected: Value at 'numbers' failed to satisfy \
                     constraint: Member must have length less than or equal to 1000; Value at \
                     'numbers' failed to satisfy constraint: {ELEMENT_SET}"
                ),
            ),
            (
                // A member the schemas declare no element constraints for keeps the
                // framework's own clause, path and echo.
                "a scalar member of the same request",
                vec![violation(
                    "name",
                    ViolationKind::Pattern(r"^[a-z]+$".to_owned()),
                    "NOPE",
                )],
                "1 validation error detected: Value 'NOPE' at 'name' failed to satisfy \
                 constraint: Member must satisfy regular expression pattern: ^[a-z]+$"
                    .to_owned(),
            ),
        ];

        for (case, violations, expected) in rows {
            let stated = aws_validation_refusal_over_elements(
                &Refusal::Invalid(violations),
                &[&INPUT],
                str::to_string,
            )
            .unwrap_or_else(|| panic!("case {case:?}: expected {expected:?}, got none"));
            assert_eq!(
                stated.code, "ValidationException",
                "case {case:?}: expected the JSON protocols' refusal code, got {:?}",
                stated.code
            );
            assert_eq!(
                stated.message, expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                stated.message
            );
        }
    }

    /// RULE: the two choices the caller-stating entry point takes COMPOSE — a violation at
    /// a collection ELEMENT is stated at the member the caller's set names, and the value
    /// half of every OTHER clause is dropped where the caller withholds it, including the
    /// `null` an absent member is otherwise stated with.
    ///
    /// The rows span what the composition generalises over: an element of a NESTED
    /// collection (the member path a caller-stated set is asked for), a scalar in another
    /// class, an absent required member, and the same refusal with nothing withheld.
    #[test]
    fn a_stated_element_set_and_a_withheld_value_compose() {
        let nested_set = element_set_requirement(&[
            requirement(ViolationKind::MaxLength(2048)),
            requirement(ViolationKind::Pattern("[a-z]+".to_owned())),
        ]);
        let refusal = Refusal::Invalid(vec![
            violation(
                "entries.1.member.recordIds.2.member",
                ViolationKind::Pattern("^[a-z]+$".to_owned()),
                "NOPE",
            ),
            violation("maxResults", ViolationKind::MaxValue("20".to_owned()), "21"),
            Violation {
                path: "entries".to_owned(),
                kind: ViolationKind::MissingRequiredMember,
                value: None,
                sensitive: false,
            },
        ]);
        // The set is asked for by the member path the clause is stated at, which is the
        // nested one a caller looks its own shapes up by.
        let element_set = |collection: &str| {
            (collection == "entries.1.member.recordIds").then(|| nested_set.clone())
        };

        let withheld = aws_validation_refusal_over_elements_stating_withholding(
            &refusal,
            crate::validation::unanchored,
            |_| true,
            element_set,
        )
        .expect("a constraint refusal is stated");
        assert_eq!(
            withheld.message,
            format!(
                "3 validation errors detected: Value at 'entries.1.member.recordIds' failed \
                 to satisfy constraint: {nested_set}; Value at 'maxResults' failed to satisfy \
                 constraint: Member must have value less than or equal to 20; Value at \
                 'entries' failed to satisfy constraint: Member must not be null"
            ),
            "expected every clause to name its member and its requirement alone, got {:?}",
            withheld.message
        );

        let quoted = aws_validation_refusal_over_elements_stating_withholding(
            &refusal,
            crate::validation::unanchored,
            |_| false,
            element_set,
        )
        .expect("a constraint refusal is stated");
        assert_eq!(
            quoted.message,
            format!(
                "3 validation errors detected: Value at 'entries.1.member.recordIds' failed \
                 to satisfy constraint: {nested_set}; Value '21' at 'maxResults' failed to \
                 satisfy constraint: Member must have value less than or equal to 20; Value \
                 null at 'entries' failed to satisfy constraint: Member must not be null"
            ),
            "expected a grouped clause to state no value while the others keep theirs, got {:?}",
            quoted.message
        );
        assert!(
            aws_validation_refusal_over_elements_stating_withholding(
                &Refusal::Unauthenticated,
                crate::validation::unanchored,
                |_| true,
                element_set,
            )
            .is_none(),
            "expected an unauthenticated caller to keep the framework's 403 wording"
        );
    }

    /// RULE: with no element set to read, nothing is grouped — the refusal is the one
    /// [`aws_validation_refusal_with`] states, clause for clause.
    #[test]
    fn an_element_whose_set_is_unknown_keeps_the_ungrouped_refusal() {
        let refusal = Refusal::Invalid(vec![
            violation(
                "numbers.1.member",
                ViolationKind::Pattern(r"^[0-9]+$".to_owned()),
                "x",
            ),
            violation("name", ViolationKind::MinLength(3), "ab"),
        ]);
        let grouped = aws_validation_refusal_over_elements(&refusal, &[], str::to_string)
            .expect("a constraint refusal is stated");
        let ungrouped = aws_validation_refusal_with(&refusal, str::to_string)
            .expect("a constraint refusal is stated");
        assert_eq!(
            grouped, ungrouped,
            "expected the ungrouped refusal, got {grouped:?}"
        );
        assert!(
            aws_validation_refusal_over_elements(&Refusal::Unauthenticated, &[], str::to_string)
                .is_none(),
            "expected an unauthenticated caller to keep the framework's 403 wording"
        );
        assert!(
            aws_validation_refusal_over_elements(
                &Refusal::Invalid(Vec::new()),
                &[&INPUT],
                str::to_string
            )
            .is_none(),
            "expected no violations to state nothing"
        );
    }

    /// RULE: the envelope a grouped refusal is stated in is the framework's own — the same
    /// count word and the same separator it renders over the same number of violations.
    #[test]
    fn the_envelope_is_the_frameworks_own() {
        for count in 1..=4usize {
            let clauses: Vec<String> = (0..count)
                .map(|index| format!("Value at 'member{index}' failed to satisfy constraint: X"))
                .collect();
            let violations: Vec<Violation> = clauses
                .iter()
                .enumerate()
                .map(|(index, _)| Violation {
                    path: format!("member{index}"),
                    kind: ViolationKind::MissingRequiredMember,
                    value: None,
                    sensitive: false,
                })
                .collect();
            // The framework's envelope over the same number of clauses, with its own
            // clause text swapped for the stated one so only the envelope is compared.
            let mut expected = envelope(&violations).expect("a violation renders");
            for (index, clause) in clauses.iter().enumerate() {
                expected = expected.replace(&violations[index].message(), clause);
            }
            let actual = envelope_over(&clauses).expect("a clause renders");
            assert_eq!(
                actual, expected,
                "case {count} clauses: expected {expected:?}, got {actual:?}"
            );
        }
        assert!(
            envelope_over(&[]).is_none(),
            "expected no clause to state nothing"
        );
    }

    /// RULE: a service that answers an error OF ITS OWN for a value one of its patterns
    /// does not admit states the parameter and the pattern that value had to match, in its
    /// own code; a refusal that is even partly about another constraint keeps the
    /// framework's envelope.
    ///
    /// The rows span what that generalises over: which pattern a violation carries (claimed
    /// or not), which constraint class the violation is, how many violations the refusal
    /// holds, and whether the rendering restates the expression or passes it through.
    #[test]
    fn a_claimed_patterns_refusal_names_the_parameter_and_its_pattern() {
        /// The rendering one service chooses: the expression with its one bounded field
        /// restated, which is what a service passes in.
        fn stated(pattern: &str) -> String {
            pattern.replace("[^:]+", "[a-z]+")
        }

        let rows: Vec<(&str, Refusal, Option<&str>)> = vec![
            (
                "a claimed pattern, restated",
                Refusal::Invalid(vec![violation(
                    "resourceArn",
                    ViolationKind::Pattern("^arn:aws:thing:[^:]+:x$".to_owned()),
                    "not-an-arn",
                )]),
                Some(
                    "The value supplied for parameter 'resourceArn' is not valid. resourceArn \
                     must match pattern ^arn:aws:thing:[a-z]+:x$",
                ),
            ),
            (
                "a claimed pattern the rendering leaves alone",
                Refusal::Invalid(vec![violation(
                    "otherArn",
                    ViolationKind::Pattern("^arn:aws:thing:x$".to_owned()),
                    "x",
                )]),
                Some(
                    "The value supplied for parameter 'otherArn' is not valid. otherArn must \
                     match pattern ^arn:aws:thing:x$",
                ),
            ),
            (
                "two claimed patterns: the first parameter is named",
                Refusal::Invalid(vec![
                    violation(
                        "firstArn",
                        ViolationKind::Pattern("^arn:aws:thing:x$".to_owned()),
                        "x",
                    ),
                    violation(
                        "secondArn",
                        ViolationKind::Pattern("^arn:aws:thing:y$".to_owned()),
                        "y",
                    ),
                ]),
                Some(
                    "The value supplied for parameter 'firstArn' is not valid. firstArn must \
                     match pattern ^arn:aws:thing:x$",
                ),
            ),
            (
                "a pattern the service does not claim",
                Refusal::Invalid(vec![violation(
                    "version",
                    ViolationKind::Pattern(r"^[0-9]+$".to_owned()),
                    "x",
                )]),
                None,
            ),
            (
                "a constraint that is not a pattern",
                Refusal::Invalid(vec![violation("name", ViolationKind::MinLength(3), "ab")]),
                None,
            ),
            (
                "one claimed pattern beside another constraint",
                Refusal::Invalid(vec![
                    violation(
                        "resourceArn",
                        ViolationKind::Pattern("^arn:aws:thing:x$".to_owned()),
                        "x",
                    ),
                    violation("name", ViolationKind::MinLength(3), "ab"),
                ]),
                None,
            ),
            ("no violations at all", Refusal::Invalid(Vec::new()), None),
            ("an unauthenticated caller", Refusal::Unauthenticated, None),
        ];

        for (case, refusal, expected) in &rows {
            let answered = parameter_pattern_refusal(
                refusal,
                "InvalidParameterValueException",
                |pattern| pattern.starts_with("^arn:aws:thing:"),
                stated,
            );
            match expected {
                Some(message) => {
                    let stated = answered
                        .unwrap_or_else(|| panic!("case {case:?}: expected a stated refusal"));
                    assert_eq!(
                        stated.code, "InvalidParameterValueException",
                        "case {case:?}: expected the service's own code, got {:?}",
                        stated.code
                    );
                    assert_eq!(
                        stated.message, *message,
                        "case {case:?}: expected {message:?}, got {:?}",
                        stated.message
                    );
                }
                None => assert!(
                    answered.is_none(),
                    "case {case:?}: expected the framework's own wording, got {answered:?}"
                ),
            }
        }
    }

    /// RULE: a member path is spelled the way the API addresses the parameter — a
    /// collection element as a zero-based bracket index on the member that holds it, every
    /// other segment as the walk spelled it.
    ///
    /// The rows span what the spelling generalises over: depth, how many collections one
    /// path steps through, where in the path the element sits, and a position that is not an
    /// element step at all.
    #[test]
    fn a_member_path_is_spelled_the_way_the_api_addresses_it() {
        // (case, the walk's path, the parameter the API names)
        let rows = [
            ("a top-level member", "semanticVersion", "semanticVersion"),
            (
                "a member of a structure member",
                "imageTestsConfiguration.timeoutMinutes",
                "imageTestsConfiguration.timeoutMinutes",
            ),
            (
                "a member inside a list element",
                "distributions.1.member.amiDistributionConfiguration.name",
                "distributions[0].amiDistributionConfiguration.name",
            ),
            (
                "a later element of a list",
                "blockDeviceMappings.4.member.ebs.volumeSize",
                "blockDeviceMappings[3].ebs.volumeSize",
            ),
            (
                "a scalar element of a list",
                "supportedOsVersions.2.member",
                "supportedOsVersions[1]",
            ),
            (
                "an element inside an element",
                "distributions.1.member.launchTemplateConfigurations.2.member.accountId",
                "distributions[0].launchTemplateConfigurations[1].accountId",
            ),
            ("a map entry's value", "amiTags.owner.member", "amiTags.owner"),
            (
                "a member whose name is a number but no element step",
                "filters.1",
                "filters.1",
            ),
        ];

        for (case, path, expected) in rows {
            let actual = api_parameter_path(path);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the parameter {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: one sentence states every parameter refusal — the parameter (or the
    /// combination), then the requirement its value had to satisfy — and the requirement
    /// clause names the member and what the model admits.
    ///
    /// The rows span the constraint classes a requirement is stated for and the two forms of
    /// the sentence.
    #[test]
    fn a_parameter_refusal_states_the_parameter_and_its_requirement() {
        // (case, the sentence, what it must read)
        let enum_values = [
            "Self".to_owned(),
            "Shared".to_owned(),
            "Amazon".to_owned(),
        ];
        let rows = [
            (
                "a pattern",
                parameter_value_mismatch(
                    "semanticVersion",
                    &must_match_pattern("semanticVersion", r"^[0-9]+\.[0-9]+\.[0-9]+$"),
                ),
                "The value supplied for parameter 'semanticVersion' is not valid. \
                 semanticVersion must match pattern ^[0-9]+\\.[0-9]+\\.[0-9]+$",
            ),
            (
                "a pattern whose expression is not stated",
                parameter_value_mismatch(
                    "distributionConfigurationArn",
                    &pattern_requirement_unstated("distributionConfigurationArn"),
                ),
                "The value supplied for parameter 'distributionConfigurationArn' is not \
                 valid. distributionConfigurationArn does not match the required pattern",
            ),
            (
                "an upper bound",
                parameter_value_mismatch(
                    "imageTestsConfiguration.timeoutMinutes",
                    &must_be_at_most("imageTestsConfiguration.timeoutMinutes", "1440"),
                ),
                "The value supplied for parameter 'imageTestsConfiguration.timeoutMinutes' \
                 is not valid. imageTestsConfiguration.timeoutMinutes must be smaller than \
                 or equal to 1440",
            ),
            (
                "a lower bound",
                parameter_value_mismatch("maxResults", &must_be_at_least("maxResults", "1")),
                "The value supplied for parameter 'maxResults' is not valid. maxResults \
                 must be greater than or equal to 1",
            ),
            (
                "an enumerated value set",
                parameter_value_mismatch("owner", &must_be_one_of("owner", &enum_values)),
                "The value supplied for parameter 'owner' is not valid. owner must be one \
                 of ['Self', 'Shared', 'Amazon']",
            ),
            (
                "a requirement over two members together",
                parameters_value_mismatch(
                    &["data", "uri"],
                    "You must specify either data or a uri.",
                ),
                "The value supplied for parameter 'data' and 'uri' is not valid. You must \
                 specify either data or a uri.",
            ),
            (
                "a requirement over three members together",
                parameters_value_mismatch(&["a", "b", "c"], "Pick one."),
                "The value supplied for parameter 'a', 'b' and 'c' is not valid. Pick one.",
            ),
        ];

        for (case, stated, expected) in &rows {
            assert_eq!(
                stated, expected,
                "case {case:?}: expected {expected:?}, got {stated:?}"
            );
        }

        // The pattern sentence a service states through the whole-refusal entry point and
        // the one built from the clauses are the SAME sentence.
        let composed = parameter_value_mismatch("arn", &must_match_pattern("arn", "^arn:"));
        let stated = parameter_pattern_mismatch("arn", "^arn:");
        assert_eq!(
            stated, composed,
            "case \"one sentence for one requirement\": expected {composed:?}, got {stated:?}"
        );
    }
}
