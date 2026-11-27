// SPDX-License-Identifier: CC0-1.0

//! What a definition is VALIDATED against: every problem one carries, where it is,
//! and whether it prevents a state machine from being created with it.
//!
//! One concern, split out of [`super`] so that what the interpreter RUNS is read
//! apart from what the service ACCEPTS. A definition this emulator cannot run is
//! still valid Amazon States Language, so this says nothing about the walk: it
//! reports the language's own structural rules, which `CreateStateMachine`,
//! `UpdateStateMachine` and `ValidateStateMachineDefinition` read through
//! [`validate`], [`validate_single_state`] and [`diagnose`].
//!
//! Every budget here is spent against a CALLER-SIZED document (the modelled
//! `Definition` is 1 MiB): the diagnostics one walk collects, the characters one
//! location carries and the states the variable-scope analysis visits each carry
//! their own ceiling, so the work of one request is never the product of the state
//! count and the nesting.

use super::*;

/// The diagnostic code `INVALID_JSON_DESCRIPTION`: "JSON syntax problem found".
///
/// The codes below are the ones this emulator's validation can reach, out of
/// the error-code list the model documents on
/// `ValidateStateMachineDefinitionDiagnostic`
/// (`smithy_query(query="doc",shape="ValidateStateMachineDefinitionDiagnostic")`).
/// That same documentation is why the code is the stable part of an
/// `InvalidDefinition` message and the detail after it is not: "might add new
/// diagnostics in the future, adjust diagnostic codes, or change the message
/// wording. […] Do not rely on the exact order, count, or wording of diagnostic
/// messages."
pub const INVALID_JSON_DESCRIPTION: &str = "INVALID_JSON_DESCRIPTION";

/// `SCHEMA_VALIDATION_FAILED`: "Schema validation reported errors" — the definition
/// is JSON, but not the shape the Amazon States Language declares.
pub const SCHEMA_VALIDATION_FAILED: &str = "SCHEMA_VALIDATION_FAILED";

/// `UNSUPPORTED_JSONATA_EXPRESSION`: the definition is the shape the language
/// declares, but one of its JSONata expressions is not one the service accepts —
/// a reserved term read where the language does not bind it, which is a problem with
/// the EXPRESSION rather than with the shape of the document around it. The model's
/// own list of codes is not closed ("might add new diagnostics in the future"), so a
/// code it does not list is still a code the service reports.
pub const UNSUPPORTED_JSONATA_EXPRESSION: &str = "UNSUPPORTED_JSONATA_EXPRESSION";

/// `INVALID_JSONATA_EXPRESSION`: a field of a JSONata state carries text the language
/// cannot PARSE at all, so no run of the definition could ever evaluate it. It is told
/// from [`UNSUPPORTED_JSONATA_EXPRESSION`], which is an expression the language parses
/// and this service does not accept there. The model's list of codes is not closed, so
/// a code it does not list is still a code the service reports.
pub const INVALID_JSONATA_EXPRESSION: &str = "INVALID_JSONATA_EXPRESSION";

/// `MISSING_TRANSITION_TARGET`: "The value of \"Next\" field doesn't match a known
/// state name" — reported for every transition target that names no declared state,
/// `StartAt`'s included.
pub const MISSING_TRANSITION_TARGET: &str = "MISSING_TRANSITION_TARGET";

/// `INVALID_STATE_NAME`: "The state name does not follow the naming convention".
pub const INVALID_STATE_NAME: &str = "INVALID_STATE_NAME";

/// `MISSING_END_STATE`: "The workflow does not have a terminal state" — no state the
/// state machine DECLARES ends an execution, so nothing it can run ever stops.
pub const MISSING_END_STATE: &str = "MISSING_END_STATE";

/// `MISSING_DESCRIPTION`: "Received a null or empty workflow input" — the request
/// carried no document to validate at all.
pub const MISSING_DESCRIPTION: &str = "MISSING_DESCRIPTION";

/// `TOO_DEEPLY_NESTED`: "The states are too deeply nested".
pub const TOO_DEEPLY_NESTED: &str = "TOO_DEEPLY_NESTED";

/// `NO_DOLLAR`: "No `.$` on a field that appears to be a JSONPath or Intrinsic
/// Function" — a WARNING, because the field is rendered as the literal it is
/// rather than refused.
pub const NO_DOLLAR: &str = "NO_DOLLAR";

/// `NO_PATH`: "Field value looks like a path, but field name does not end with
/// 'Path'" — a WARNING, for the same reason.
pub const NO_PATH: &str = "NO_PATH";

/// `PASS_RESULT_IS_STATIC`: "Attempt to use a path in the result of a pass state"
/// — a WARNING: a `Pass` state's `Result` is its literal result.
pub const PASS_RESULT_IS_STATIC: &str = "PASS_RESULT_IS_STATIC";

/// `POSSIBLY_UNDEFINED_VARIABLE`: a JSONata expression reads a variable that some run
/// of the definition can reach that expression without having assigned — a WARNING,
/// because the expression EVALUATES to nothing rather than preventing the definition
/// from being created. It is told from [`UNDEFINED_VARIABLE`], which is the same read
/// at a field the language RESOLVES a path at. The model's list of codes is not closed
/// ("might add new diagnostics in the future"), so a code it does not list is still a
/// code the service reports.
pub const POSSIBLY_UNDEFINED_VARIABLE: &str = "POSSIBLY_UNDEFINED_VARIABLE";

/// `UNDEFINED_VARIABLE`: a field the language RESOLVES a path at names a variable the
/// scope reaching that field may not hold — an ERROR, because the reference is resolved
/// rather than evaluated, so there is nothing for the language to resolve it against.
/// The model's list of codes is not closed, so a code it does not list is still a code
/// the service reports.
pub const UNDEFINED_VARIABLE: &str = "UNDEFINED_VARIABLE";

/// The most diagnostics `ValidateStateMachineDefinition` reports in one call, which
/// is also what it reports when the request asks for none in particular
/// (`maxResults`: "The maximum number of diagnostics that are returned per call.
/// The default and maximum value is 100. Setting the value to 0 will also use the
/// default of 100.").
pub const DEFAULT_MAX_DIAGNOSTICS: usize = 100;

/// How many diagnostics of ONE severity a walk collects.
///
/// A definition is caller-sized (the modelled `Definition` is 1 MiB), so the list
/// this walk builds carries its own ceiling like every other walk here. It is one
/// past [`DEFAULT_MAX_DIAGNOSTICS`], which is `maxResults`' modelled maximum: no
/// request can ask for more than that, so one more than the maximum is all it takes
/// to report `truncated`.
///
/// The budget is per SEVERITY, and the walk stops only once the ERROR budget is
/// spent: a definition's warnings can therefore never crowd out an error, which is
/// what [`validate`] — and so what `CreateStateMachine` refuses — reads.
pub const MAX_DIAGNOSTICS: usize = DEFAULT_MAX_DIAGNOSTICS + 1;

/// How many of a definition's JSONata expressions ONE walk PARSES.
///
/// A definition is caller-sized and every field of a JSONata state may carry an
/// expression, and each parse is recursive descent over a caller's text on a stack
/// sized for it, so the work one request can ask for is fixed here rather than by the
/// number of such fields the document declares. It is [`MAX_DIAGNOSTICS`], the most
/// problems of one severity a walk reports at all: a walk that has spent it could
/// report no further parse failure anyway.
pub const MAX_PARSED_EXPRESSIONS: usize = MAX_DIAGNOSTICS;

/// The most characters of a caller's own value one diagnostic echoes.
///
/// A definition is caller-sized and its member names, state names and string values
/// are caller-chosen, so a message that echoed one whole would let one request be
/// amplified into a much larger response. The cap is the longest a state name may
/// be ([`MAX_STATE_NAME_CHARS`]), so a value inside the language's own bounds is
/// quoted in full and only one past them is clipped.
pub const MAX_ECHOED_CHARS: usize = MAX_STATE_NAME_CHARS;
/// The most characters one diagnostic's `location` carries where that location is a
/// PATH into the definition, and the most any path the walk builds carries: the path
/// to a state nests a caller-chosen name per level, so it is clipped AS IT IS BUILT
/// ([`nested_at`]) rather than only where it is reported.
///
/// ONE diagnostic is located somewhere else: a definition that is not JSON at all has
/// no path into it, so it is located at the position the parse ended at
/// ([`crate::state::json_parse_definition_location`]), whose own quoting of the
/// document is what bounds it.
pub const MAX_LOCATION_CHARS: usize = 256;

/// `value` as a diagnostic echoes it: itself when it is short, and its first
/// [`MAX_ECHOED_CHARS`] characters followed by an ellipsis when it is not.
///
/// Public so the one clipping of a quoted caller value serves every cause this
/// crate words, including the ones [`super::super::integration`] builds for a `Task`
/// state's `Resource`.
pub fn echoed(value: &str) -> String {
    clipped(value, MAX_ECHOED_CHARS)
}

/// `value` clipped to `cap` characters, with an ellipsis marking what was dropped.
///
/// The ellipsis is one of the `cap`, so the result never carries more characters
/// than the cap allows.
///
/// Not [`super::clip_cause`], which is the other truncation this module does and a
/// different one: that cuts a `cause` to the BYTES the modelled `SensitiveCause`
/// carries and must leave the text it keeps exactly as the run produced it, since
/// it is a reported value rather than a quotation of one.
pub fn clipped(value: &str, cap: usize) -> String {
    if value.chars().count() <= cap {
        return value.to_string();
    }
    let kept: String = value.chars().take(cap.saturating_sub(1)).collect();
    format!("{kept}\u{2026}")
}

/// One problem found in a definition: what it is, where it is, and whether it
/// prevents creating or updating a state machine with that definition.
///
/// `severity` is [`crate::state::SEVERITY_ERROR`] or
/// [`crate::state::SEVERITY_WARNING`], `code` one of the codes above, and
/// `location` the `/States/<StateName>/<FieldName>` form the `location` member of
/// `ValidateStateMachineDefinitionDiagnostic` documents, for a diagnostic a field
/// or a state owns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: &'static str,
    pub code: &'static str,
    pub message: String,
    pub location: Option<String>,
}

/// The place an `InvalidDefinition` subject states for a diagnostic that sits at no
/// position in the definition ([`Diagnostic::stated`]).
///
/// RECORDED text: the refusal states the place of every subject, and for a diagnostic
/// about the definition as a whole that place is the bare word `null`.
const NO_LOCATION_STATED: &str = "null";

impl Diagnostic {
    /// Whether this diagnostic prevents a create or an update: "A value of `ERROR`
    /// means that you cannot create or update a state machine with this
    /// definition."
    pub fn is_error(&self) -> bool {
        self.severity == crate::state::SEVERITY_ERROR
    }

    /// This diagnostic as the SUBJECT of an `InvalidDefinition` message: the code
    /// that classifies it, the detail describing it, and where it was found.
    ///
    /// The place is joined to the detail by the bare word ` at ` and nothing else, so
    /// the subject reads as one sentence and a client that parses the location out of
    /// a refusal finds it unwrapped.
    ///
    /// EVERY subject states a place, including a diagnostic about the definition AS A
    /// WHOLE — a workflow that declares no terminal state is the recorded one — which
    /// sits at no position in the document: that place is stated as
    /// [`NO_LOCATION_STATED`], the way the refusal RECORDS it, so a client parsing the
    /// location out of a subject finds the same field in every one of them. The
    /// `location` MEMBER of a `ValidateStateMachineDefinition` diagnostic is a
    /// different report of the same fact and stays absent there.
    pub(super) fn stated(&self) -> String {
        let location = self.location.as_deref().unwrap_or(NO_LOCATION_STATED);
        format!("{}: {} at {location}", self.code, self.message)
    }
}

/// One diagnostic of the given severity.
pub fn diagnostic(
    severity: &'static str,
    code: &'static str,
    location: Option<String>,
    message: impl ::std::fmt::Display,
) -> Diagnostic {
    Diagnostic {
        severity,
        code,
        message: message.to_string(),
        location,
    }
}

/// One ERROR diagnostic: a definition carrying it cannot be created or updated.
pub fn error(code: &'static str, location: Option<String>, message: impl ::std::fmt::Display) -> Diagnostic {
    diagnostic(crate::state::SEVERITY_ERROR, code, location, message)
}

/// One WARNING diagnostic: a definition carrying it is still created.
pub fn warning(code: &'static str, location: Option<String>, message: impl ::std::fmt::Display) -> Diagnostic {
    diagnostic(crate::state::SEVERITY_WARNING, code, location, message)
}

/// Where one diagnostic was found: the enclosing machine's own location followed
/// by `member`, or `None` for the definition as a whole ([`Diagnostic::stated`] is
/// what a refusal states an absent one as).
///
/// `prefix` is itself bounded (every caller builds one through [`nested_at`]), so
/// the only caller-sized part of a location is the one member named here.
pub fn at(prefix: &str, member: &str) -> Option<String> {
    let location = format!("{prefix}{member}");
    (!location.is_empty()).then(|| clipped(&location, MAX_LOCATION_CHARS))
}

/// The location to hand to something nested inside `prefix`: `prefix` followed by
/// `member`, clipped AS IT IS BUILT.
///
/// The path to a state nests one caller-chosen name per level and is then built
/// once per state, so a path that grew with the definition would make the work of
/// one request the PRODUCT of the state count and the nesting; clipping here is
/// what keeps every path handed down O([`MAX_LOCATION_CHARS`]).
pub fn nested_at(prefix: &str, member: impl ::std::fmt::Display) -> String {
    clipped(&format!("{prefix}{member}"), MAX_LOCATION_CHARS)
}

/// The location of the element at `index` of the array `array` locates: the array's
/// own location with the index BRACKETED onto it.
///
/// RECORDED form, and the ONE place an index is written into a location, so every
/// diagnostic about an element of a `Retry`, `Catch`, `Choices`, `Branches` or
/// `ErrorEquals` array addresses it the same way. The index belongs to the FIELD that
/// holds the array rather than being a segment of its own, so a client reading the
/// location to point at the offending element of the document it submitted resolves
/// the node the diagnostic is about.
pub fn indexed_at(array: &str, index: usize) -> String {
    nested_at(array, format_args!("[{index}]"))
}

/// The location of the element at `index` of the array held by `member` of the object
/// at `prefix` — [`indexed_at`] of that member's own location.
pub fn element_at(prefix: &str, member: impl ::std::fmt::Display, index: usize) -> String {
    indexed_at(&nested_at(prefix, format_args!("/{member}")), index)
}

/// Where a diagnostic about the state machine OBJECT itself was found: the object's
/// own location, which for the definition as a whole is the document's root.
///
/// [`at`] answers `None` for the root, because a diagnostic about a MEMBER of the
/// root carries that member's name; a diagnostic about the object carries the object,
/// and the root object is `/`.
pub fn machine_at(prefix: &str) -> Option<String> {
    Some(if prefix.is_empty() {
        "/".to_string()
    } else {
        clipped(prefix, MAX_LOCATION_CHARS)
    })
}

/// The fields the Amazon States Language declares on a state machine object, in the
/// order its schema states them.
///
/// From the language specification's "State Machine Structure": a state machine
/// carries `Comment`, `StartAt`, `TimeoutSeconds`, `Version`, `States` and
/// `QueryLanguage`. The set is CLOSED — a field outside it is a field the language
/// does not define — which is what [`field_not_supported_message`] reports.
const MACHINE_FIELDS: &[&str] = &[
    "Comment",
    "QueryLanguage",
    "StartAt",
    "States",
    "TimeoutSeconds",
    "Version",
];

/// The versions of the Amazon States Language a state machine object may declare
/// itself written in.
///
/// From the language specification's "State Machine Structure": `Version` is "the
/// version of the States language used in the state machine (default is \"1.0\")", and
/// 1.0 is the only version the language defines, so the set is a one-entry enumeration
/// — which is what [`one_of_values_message`] reports a value outside against.
const LANGUAGE_VERSIONS: &[&str] = &["1.0"];

/// What a schema diagnostic says about a member whose value is outside the set the
/// language admits there: the values it admits, in brackets, as a list.
///
/// RECORDED text. The sentence names the admitted values and nothing else — the
/// rejected one is not echoed, exactly as [`STATE_TYPE_VALUES_MESSAGE`] does not echo a
/// `Type` the language does not define, and WHICH member carried it is what `location`
/// says.
pub fn one_of_values_message(values: &[&str]) -> String {
    format!("Value should be one of the following: [{}]", values.join(", "))
}

/// The fields a state machine declared as a `Map` state's processor carries: the
/// machine's own, and the `ProcessorConfig` that says how that state runs it.
///
/// A processor is the one position the language declares that field at, so a
/// definition or a `Parallel` branch carrying it is carrying a field the language
/// does not declare THERE — which is what [`machine_fields`] keys on.
const PROCESSOR_FIELDS: &[&str] = &[
    "Comment",
    "ProcessorConfig",
    "QueryLanguage",
    "StartAt",
    "States",
    "TimeoutSeconds",
    "Version",
];

/// Which closed set a state machine object's fields are judged against: a `Map`
/// state's processor admits one field more than a definition or a branch does.
pub fn machine_fields(processor: bool) -> &'static [&'static str] {
    if processor {
        PROCESSOR_FIELDS
    } else {
        MACHINE_FIELDS
    }
}

/// The fields a state machine object must carry, in the order the refusal lists them.
///
/// "A State Machine MUST have an object field named `States`" and "a string field
/// named `StartAt`", which are the two the schema requires; the order here is the
/// order the service lists them in, and it is the LIST that is reported rather than
/// one diagnostic per field.
pub const MACHINE_REQUIRED_FIELDS: &[&str] = &["States", "StartAt"];

/// What a schema diagnostic says about required fields an object does not carry: the
/// fields, in brackets, as a list.
///
/// RECORDED text. One diagnostic names them all, so a caller is told everything the
/// object owes rather than one field per attempt.
pub fn required_fields_message(fields: &[&str]) -> String {
    format!("These fields are required: [{}]", fields.join(", "))
}

/// What a schema diagnostic says about a field the language does not declare: the
/// field, quoted, and that it is not supported.
///
/// RECORDED text. The field name is the caller's own JSON key, so it is clipped by
/// [`echoed`] like every other caller value a diagnostic quotes.
pub fn field_not_supported_message(field: &str) -> String {
    format!("Field '{}' is not supported", echoed(field))
}

/// [`field_not_supported_message`], followed by the field the definition probably
/// MEANT where the one it wrote is a near miss of a field `declared` names.
///
/// A field the language does not declare is most often a misspelling of one it does,
/// and the service says which: a caller is told the field it meant rather than only
/// that the one it wrote is unknown. The opening is the RECORDED sentence
/// ([`field_not_supported_message`]); the suggestion after it is this service's own
/// wording for the hint the issue records it giving — no recording of that half is at
/// hand, so a later reader can tell it from the sentence before it. The suggestion is
/// the field's own name, read off the closed set the object is judged against, so every
/// position with such a set — a state machine object, a state, a retrier, a catcher, a
/// `Choice` rule, a `Map`'s own objects — suggests out of its OWN fields and no list of
/// names is written for it.
pub fn unsupported_field_message(field: &str, declared: &[&str]) -> String {
    match near_miss(field, declared) {
        Some(meant) => format!("{}. Did you mean '{meant}'?", field_not_supported_message(field)),
        None => field_not_supported_message(field),
    }
}

/// The field of `declared` that `field` is a NEAR MISS of, or `None` when it is a near
/// miss of none of them.
///
/// A near miss is a name ONE edit away from a declared field — a character changed,
/// one added, one dropped or two adjacent ones swapped — or one that differs from it in
/// CASE alone, which are the ways a field name is mistyped. The first such field of the
/// set is suggested; the sets are written in the order the language's schema states
/// them, so a name that is one edit from two of them suggests the earlier.
///
/// Each comparison is refused before it is made for a name whose LENGTH is more than
/// one away from the declared field's ([`differs_by_one_edit`]), so the work of judging
/// one caller-chosen key is bounded by the declared names rather than by the key: no
/// definition can ask for a comparison of its own size.
fn near_miss<'a>(field: &str, declared: &[&'a str]) -> Option<&'a str> {
    declared
        .iter()
        .copied()
        .find(|known| differs_by_one_edit(field, known))
}

/// Whether `written` is ONE edit away from `declared`, or differs from it only in case.
///
/// Four edits, which are the ways one name is mistyped: a character substituted, one
/// inserted, one dropped, and two adjacent ones transposed. A name EQUAL to the
/// declared field is no misspelling of it, so it is not one.
fn differs_by_one_edit(written: &str, declared: &str) -> bool {
    if written == declared {
        return false;
    }
    if written.eq_ignore_ascii_case(declared) {
        return true;
    }
    // A name whose length is more than one from the declared field's cannot be one
    // edit from it, which is also what keeps the walk below bounded by the DECLARED
    // name rather than by the caller's key.
    let written: Vec<char> = written.chars().take(MAX_ECHOED_CHARS + 1).collect();
    let declared: Vec<char> = declared.chars().collect();
    if written.len().abs_diff(declared.len()) > 1 {
        return false;
    }
    let Some(first) = (0..written.len().min(declared.len()))
        .find(|index| written[*index] != declared[*index])
    else {
        // One is a prefix of the other, and their lengths differ by one: the longer
        // carries one character the shorter does not.
        return true;
    };
    match written.len().cmp(&declared.len()) {
        // The same length: the character at the mismatch was substituted, or it and
        // the one after it were swapped.
        ::std::cmp::Ordering::Equal => {
            written[first + 1..] == declared[first + 1..]
                || (written.get(first + 1) == declared.get(first)
                    && written.get(first) == declared.get(first + 1)
                    && written[first + 2..] == declared[first + 2..])
        }
        // One character longer: it carries one the declared field does not.
        ::std::cmp::Ordering::Greater => written[first + 1..] == declared[first..],
        // One character shorter: it is missing one the declared field carries.
        ::std::cmp::Ordering::Less => written[first..] == declared[first + 1..],
    }
}

/// The fields the Amazon States Language declares on ONE rule of a `Choice` state's
/// `Choices` array, in the order its schema states them.
///
/// The UNION of what the two query languages declare for a rule, exactly as a state's
/// own set is ([`state_fields`]): a JSONPath rule names the value it tests (`Variable`)
/// and ONE comparator, a JSONata rule carries a `Condition`, and either may nest rules
/// under `And`, `Or` and `Not`, say where the execution goes (`Next`), assign variables
/// and build its own output. The comparators are the language's own catalogue — the
/// three string, numeric, boolean and timestamp families, each with the `*Path` form
/// that READS its operand from the state's input, the `StringMatches` glob, and the
/// presence and type tests — from the Amazon States Language's "Choice Rules"; the set
/// is CLOSED, so a name outside it is reported as the unsupported field it is
/// ([`unsupported_field_message`], which suggests the comparator a misspelling of one
/// of them was meant to be).
const CHOICE_RULE_FIELDS: &[&str] = &[
    "And",
    "Assign",
    "BooleanEquals",
    "BooleanEqualsPath",
    "Comment",
    "Condition",
    "IsBoolean",
    "IsNull",
    "IsNumeric",
    "IsPresent",
    "IsString",
    "IsTimestamp",
    "Next",
    "Not",
    "NumericEquals",
    "NumericEqualsPath",
    "NumericGreaterThan",
    "NumericGreaterThanEquals",
    "NumericGreaterThanEqualsPath",
    "NumericGreaterThanPath",
    "NumericLessThan",
    "NumericLessThanEquals",
    "NumericLessThanEqualsPath",
    "NumericLessThanPath",
    "Or",
    "Output",
    "StringEquals",
    "StringEqualsPath",
    "StringGreaterThan",
    "StringGreaterThanEquals",
    "StringGreaterThanEqualsPath",
    "StringGreaterThanPath",
    "StringLessThan",
    "StringLessThanEquals",
    "StringLessThanEqualsPath",
    "StringLessThanPath",
    "StringMatches",
    "TimestampEquals",
    "TimestampEqualsPath",
    "TimestampGreaterThan",
    "TimestampGreaterThanEquals",
    "TimestampGreaterThanEqualsPath",
    "TimestampGreaterThanPath",
    "TimestampLessThan",
    "TimestampLessThanEquals",
    "TimestampLessThanEqualsPath",
    "TimestampLessThanPath",
    "Variable",
];

/// The members of a `Choice` rule that carry RULES of their own, and so are walked as
/// rules rather than read as a comparator's operand.
const NESTED_RULE_MEMBERS: &[&str] = &["And", "Not", "Or"];

/// The member a `Choice` rule says where the execution goes with, which is the field
/// every rule of a `Choice` state's own `Choices` array owes: "Each Choice Rule […] MUST
/// have a \"Next\" field, whose value MUST match a state name".
const CHOICE_TRANSITION_MEMBER: &str = "Next";

/// What a diagnostic says about a rule of a `Choice` state that points the execution
/// nowhere: that the transition the rule owes is missing.
///
/// RECORDED text. It has no variable part — WHICH rule of which state left the member
/// out is what `location` carries — and it is told from [`missing_target_message`], which
/// is the other defect of the transition graph: a target that IS named and resolves to no
/// state the machine declares.
pub const MISSING_TRANSITION_POINTER_MESSAGE: &str = "Missing transition pointer";

/// The fields the Amazon States Language declares on a state of each type, in the
/// order its schema states them.
///
/// Each set is CLOSED, exactly as a state machine object's is ([`machine_fields`]): a
/// state's `Type` selects what the state DOES, and the language declares per type the
/// fields that say how it does it, so a field outside the set of the type the state
/// declares is a field the language does not define THERE —
/// [`field_not_supported_message`] is what reports it. A type the language does not
/// define at all carries no set, because nothing says what such a state would do; it
/// is reported as the type it names instead ([`diagnose_state`]).
///
/// Each set is the UNION of what the two query languages declare for the type — the
/// `*Path` family and the payload templates of JSONPath beside `Arguments`, `Output`
/// and `Assign` of JSONata ([`super::executable_members`] and
/// [`super::jsonata_members`] are the same split for what this emulator RUNS) — plus
/// the fields the language declares that this emulator does not execute, since a
/// definition using one is still valid Amazon States Language. Which language's
/// members a state may MIX is a different rule, reported on its own
/// ([`diagnose_jsonata_state`]).
pub fn state_fields(state_type: &str) -> &'static [&'static str] {
    match state_type {
        "Pass" => &[
            "Assign",
            "Comment",
            "End",
            "InputPath",
            "Next",
            "Output",
            "OutputPath",
            "Parameters",
            "QueryLanguage",
            "Result",
            "ResultPath",
            "Type",
        ],
        "Wait" => &[
            "Assign",
            "Comment",
            "End",
            "InputPath",
            "Next",
            "Output",
            "OutputPath",
            "QueryLanguage",
            "Seconds",
            "SecondsPath",
            "Timestamp",
            "TimestampPath",
            "Type",
        ],
        // A `Choice` goes where its own rules send it, so it declares neither `Next`
        // nor `End`, and it produces no result of its own, so none of the result-side
        // members either.
        "Choice" => &[
            "Assign",
            "Choices",
            "Comment",
            "Default",
            "InputPath",
            "Output",
            "OutputPath",
            "QueryLanguage",
            "Type",
        ],
        // The two types that END an execution: there is nowhere for them to go, and no
        // failure of theirs for a retrier or a catcher to take over. The output a
        // `Succeed` puts out is the execution's own. Neither declares `Assign`: the
        // field is for "assigning values to variables" a LATER state reads, and nothing
        // runs after a state that ends the execution.
        "Succeed" => &[
            "Comment",
            "InputPath",
            "Output",
            "OutputPath",
            "QueryLanguage",
            "Type",
        ],
        "Fail" => &[
            "Cause",
            "CausePath",
            "Comment",
            "Error",
            "ErrorPath",
            "QueryLanguage",
            "Type",
        ],
        // The three types whose work can FAIL — a `Task`'s invocation, and the
        // machines a `Map` and a `Parallel` run — are the ones that declare `Retry`
        // and `Catch`.
        "Task" => &[
            "Arguments",
            "Assign",
            "Catch",
            "Comment",
            "Credentials",
            "End",
            "HeartbeatSeconds",
            "HeartbeatSecondsPath",
            "InputPath",
            "Next",
            "Output",
            "OutputPath",
            "Parameters",
            "QueryLanguage",
            "Resource",
            "ResultPath",
            "ResultSelector",
            "Retry",
            "TimeoutSeconds",
            "TimeoutSecondsPath",
            "Type",
        ],
        "Parallel" => &[
            "Arguments",
            "Assign",
            "Branches",
            "Catch",
            "Comment",
            "End",
            "InputPath",
            "Next",
            "Output",
            "OutputPath",
            "Parameters",
            "QueryLanguage",
            "ResultPath",
            "ResultSelector",
            "Retry",
            "Type",
        ],
        "Map" => &[
            "Assign",
            "Catch",
            "Comment",
            "End",
            "InputPath",
            "ItemBatcher",
            "ItemProcessor",
            "ItemReader",
            "ItemSelector",
            "Items",
            "ItemsPath",
            "Iterator",
            "Label",
            "MaxConcurrency",
            "MaxConcurrencyPath",
            "Next",
            "Output",
            "OutputPath",
            "Parameters",
            "QueryLanguage",
            "ResultPath",
            "ResultSelector",
            "ResultWriter",
            "Retry",
            "ToleratedFailureCount",
            "ToleratedFailureCountPath",
            "ToleratedFailurePercentage",
            "ToleratedFailurePercentagePath",
            "Type",
        ],
        _ => &[],
    }
}

/// The fields the Amazon States Language requires of a state of each type, as
/// ALTERNATIVES: a state carries the first field of each entry, or one of the fields
/// beside it.
///
/// Every state names what it does with `Type`, and a type whose work ONE field declares
/// requires that field too: a `Task` "requires a `Resource` field", a `Choice` carries
/// the `Choices` array its rules live in, a `Parallel` the `Branches` it runs, and a
/// `Map` the `ItemProcessor` its items go through (the language's earlier `Iterator`
/// names the same work). A `Pass`, `Succeed`, `Fail` or `Wait` state declares its work
/// through fields none of which it must carry alone, and a type the language does not
/// define at all requires nothing, because nothing says what such a state would do.
///
/// This is the ONE table the service holds a state to the fields its type owes with:
/// the walk reports a state missing one ([`diagnose_state`], hence every create,
/// update and validation of a DEFINITION), and [`super::validate_single_state`] reports
/// the same field for the lone state `TestState` is handed.
pub fn state_required_fields(state_type: &str) -> &'static [&'static [&'static str]] {
    match state_type {
        "Task" => &[&["Resource"]],
        "Choice" => &[&["Choices"]],
        "Parallel" => &[&["Branches"]],
        "Map" => &[PROCESSOR_FIELDS_OF_A_MAP],
        _ => &[],
    }
}

/// The requirements of `state_type` the state does not satisfy: the entries of
/// [`state_required_fields`] none of whose fields the state carries.
pub fn missing_state_fields<'a>(
    state_type: &str,
    carried: impl Fn(&str) -> bool + 'a,
) -> Vec<&'static [&'static str]> {
    state_required_fields(state_type)
        .iter()
        .copied()
        .filter(|alternatives| !alternatives.iter().any(|field| carried(field)))
        .collect()
}

/// What a schema diagnostic says about a requirement a state does not satisfy: the
/// field it owes, as the one-field list [`required_fields_message`] states.
///
/// RECORDED sentence, the one every missing field of an object is reported with. Where
/// the language declares the same work under more than one name, the field named is the
/// FIRST of the entry — the name the language declares that work under now — since that
/// is the field a caller is told to add.
pub fn missing_fields_message(alternatives: &[&str]) -> String {
    required_fields_message(&alternatives[..alternatives.len().min(1)])
}

/// Where the state `name` of the state machine at `prefix` is: the `/States/<StateName>`
/// pointer the `location` member of `ValidateStateMachineDefinitionDiagnostic`
/// documents ("the location could be in the format /States/<StateName>/<FieldName>").
///
/// The name is the one the definition DECLARES, so the pointer resolves the state in
/// the submitted document; it is bounded by the ceiling every path the walk builds
/// carries ([`nested_at`]) rather than by [`MAX_ECHOED_CHARS`], which bounds a caller
/// value quoted inside a sentence about something else.
pub fn state_pointer(prefix: &str, name: &str) -> String {
    nested_at(prefix, format_args!("/States/{name}"))
}

/// What a diagnostic says about a state whose name is longer than the Amazon States
/// Language admits ([`MAX_STATE_NAME_CHARS`], which is also the modelled `StateName`'s
/// maximum): the offending name and the limit it is past.
///
/// RECORDED text. The name is the only variable part of it, and it is quoted as the
/// definition DECLARES it rather than shortened: this diagnostic is ABOUT that name, so
/// a sentence that cut it short would not name its own subject. It is bounded like the
/// pointer reported beside it ([`MAX_LOCATION_CHARS`]), so the two name the same text.
pub fn state_name_too_long_message(name: &str) -> String {
    format!(
        "Invalid State name: State '{}' exceeds the {MAX_STATE_NAME_CHARS}-character limit \
         allowed by the service",
        clipped(name, MAX_LOCATION_CHARS)
    )
}

/// The member every state names what it DOES with, which is the one field a state of
/// any type owes.
pub const STATE_TYPE_MEMBER: &str = "Type";

/// The member a `Task` state names its work with: "a Task state must set the Resource
/// field to the ARN of the activity or task".
pub const RESOURCE_MEMBER: &str = "Resource";

/// What a schema diagnostic says about a state whose `Type` is not one of the state
/// types the Amazon States Language defines: the member, and the values it admits.
///
/// RECORDED text, bracketed list and ORDER included, and it is the whole sentence: the
/// rejected value is not echoed, so what a caller is told is which values the member
/// admits. The order is the service's own and not the sorted [`STATE_TYPES`], so the
/// list is written here as the diagnostic states it.
pub const STATE_TYPE_VALUES_MESSAGE: &str = "The field 'Type' should have one of these values: \
     [Task, Wait, Pass, Succeed, Fail, Choice, Parallel, Map]";

/// What a diagnostic says about a transition target no state declares: the name it
/// named, as the value of the transition field the language calls `Next`.
///
/// RECORDED text. One construction serves every member a transition is named
/// through — a state's own `Next`, a `Choice` rule's, a `Catch` catcher's and the
/// `Default` beside them, and the machine's own `StartAt` — because they are one
/// problem: a name that resolves to no state of the enclosing `States` map. The field
/// the sentence names is `Next` whichever member declared the transition, as the code
/// it is reported under documents ("The value of \"Next\" field doesn't match a known
/// state name"); WHICH member that was is what `location` carries. The target is the
/// caller's own string, so it is clipped by [`echoed`].
pub fn missing_target_message(target: &str) -> String {
    format!("Missing 'Next' target: {}", echoed(target))
}

/// What a diagnostic says about a state nothing in the definition leads to: the
/// state, quoted, and that nothing reaches it.
///
/// RECORDED text. It is reported under the same code an unresolved transition target
/// is, because it is the other half of one rule about the transition graph: a state
/// machine's states and the transitions between them must agree, so a name that
/// resolves to no state and a state no name resolves to are both reported. The name is
/// the caller's own, so it is clipped by [`echoed`].
pub fn unreachable_state_message(name: &str) -> String {
    format!("State \"{}\" is not reachable.", echoed(name))
}

/// The reserved error name that stands for every error a state can report.
pub const ALL_ERRORS: &str = "States.ALL";

/// The fields the Amazon States Language declares on one `Retry` retrier, in the
/// order its schema states them.
///
/// From the language's own error-handling rules: a retrier names the errors it takes
/// over (`ErrorEquals`) and how the attempts it owes are spaced (`IntervalSeconds`,
/// `MaxAttempts`, `BackoffRate`, `MaxDelaySeconds`, `JitterStrategy`). The set is
/// CLOSED, like every other object's here.
const RETRIER_FIELDS: &[&str] = &[
    "BackoffRate",
    "Comment",
    "ErrorEquals",
    "IntervalSeconds",
    "JitterStrategy",
    "MaxAttempts",
    "MaxDelaySeconds",
];

/// The field a retrier must carry: the errors it takes over.
const RETRIER_REQUIRED_FIELDS: &[&str] = &["ErrorEquals"];

/// The fields the language declares on one `Catch` catcher: the errors it takes over,
/// where the execution continues, and what it puts out on the way there.
const CATCHER_FIELDS: &[&str] = &[
    "Assign",
    "Comment",
    "ErrorEquals",
    "Next",
    "Output",
    "ResultPath",
];

/// The fields a catcher must carry, in the order the refusal lists them: the errors it
/// takes over, and the state the execution continues at.
const CATCHER_REQUIRED_FIELDS: &[&str] = &["ErrorEquals", "Next"];

/// What a schema diagnostic says about an `ErrorEquals` list naming [`ALL_ERRORS`]
/// beside other error names.
///
/// RECORDED text. The language admits the catch-all only as the single entry of a
/// retrier's or a catcher's `ErrorEquals`, since a list that holds it matches every
/// error already and the names beside it can never decide anything.
pub const ALL_ERRORS_ALONE_MESSAGE: &str = "States.ALL must appear alone and at end of list";

/// What a schema diagnostic says about a member the language declares as a LIST that
/// a definition carries as something else: the member, and what it holds.
///
/// The sentence is this service's own, as every diagnostic about the SHAPE of a value
/// is: what is recorded is the wording for a field the language does not declare at
/// all ([`field_not_supported_message`]) and for the fields an object owes
/// ([`required_fields_message`]); a value of the wrong shape is stated as the rule it
/// broke, in the words this module states every other such rule in.
pub fn array_member_message(member: &str) -> String {
    format!("'{}' must be an array", echoed(member))
}

/// What a schema diagnostic says about an entry of such a list that is not the object
/// the language declares there.
pub fn entry_object_message(member: &str) -> String {
    format!("every entry of '{}' must be a JSON object", echoed(member))
}

/// What a schema diagnostic says about a value the language requires to carry
/// something and the definition left EMPTY — an `ErrorEquals` that names no error, a
/// `States` map that declares no state.
///
/// RECORDED text, and it has no variable part: WHICH value was left empty is what the
/// diagnostic's `location` carries ("For errors specific to a field, the location
/// could be in the format: `/States/<StateName>/<FieldName>`"), so one sentence serves
/// every field of the schema that may not be empty.
pub const EMPTY_VALUE_MESSAGE: &str = "Value cannot be empty";

/// What a schema diagnostic says about a figure the definition wrote BELOW the
/// smallest value the language documents for it: that smallest value, as the language
/// writes it.
///
/// RECORDED text. The sentence names the bound and nothing else — which figure broke
/// it is what `location` carries, exactly as [`EMPTY_VALUE_MESSAGE`]'s does.
pub fn minimum_value_message(minimum: &str) -> String {
    format!("Minimum value is {minimum}")
}

/// What a schema diagnostic says about a figure the definition wrote ABOVE the largest
/// value the language documents for it.
///
/// The mirror of [`minimum_value_message`], worded like it and in the same place: a
/// bound is a bound whichever end of the range it is, so the two sentences differ only
/// in the end they name.
pub fn maximum_value_message(maximum: &str) -> String {
    format!("Maximum value is {maximum}")
}

/// One figure the Amazon States Language BOUNDS: the member it is written at, and the
/// ends of the range the language documents for it, each paired with that end as the
/// language writes it (which is what the diagnostic about it states).
///
/// A value outside either end is a schema error of the object that carries it,
/// reported at the figure itself ([`diagnose_figures`]): the definition declares work
/// the service cannot do — a wait of a negative length, a retry interval that goes
/// backwards, a concurrency below none at all — so it is refused where it is submitted
/// rather than failing every execution of the state machine it would have created.
struct Bounded {
    member: &'static str,
    minimum: Option<(f64, &'static str)>,
    maximum: Option<(f64, &'static str)>,
}

/// A figure bounded BELOW only, which is how the language documents most of them.
const fn at_least(member: &'static str, minimum: f64, stated: &'static str) -> Bounded {
    Bounded {
        member,
        minimum: Some((minimum, stated)),
        maximum: None,
    }
}

/// The figures a `Wait` state declares.
///
/// "Seconds: A time, in seconds, to wait before beginning the state specified in the
/// Next field" — a duration, so it is not negative, and a wait of none is a wait the
/// language admits.
const WAIT_FIGURES: &[Bounded] = &[at_least("Seconds", 0.0, "0")];

/// The figures a `Task` state declares.
///
/// "TimeoutSeconds: If the task runs longer than the specified seconds, this state
/// fails" and "HeartbeatSeconds: If more time than the specified seconds elapses
/// between heartbeats from the task, this state fails" — each a WINDOW the work has to
/// finish something inside, which the language documents as a positive, non-zero number
/// of seconds: a window of none at all admits no work, so the smallest either carries is
/// [`MIN_WINDOW_SECONDS`].
const TASK_FIGURES: &[Bounded] = &[
    at_least(
        "TimeoutSeconds",
        MIN_WINDOW_SECONDS,
        MIN_WINDOW_SECONDS_STATED,
    ),
    at_least(
        "HeartbeatSeconds",
        MIN_WINDOW_SECONDS,
        MIN_WINDOW_SECONDS_STATED,
    ),
];

/// The smallest a WINDOW the language documents as a positive, non-zero number of
/// seconds carries: the seconds a `Task`'s work has to finish in and to beat between
/// heartbeats, and the seconds an execution of the state machine has to run in.
///
/// A window of none at all is not a shorter deadline but work the service could never
/// perform, so the language admits none: it is told from the figures whose own documented
/// default IS none — a `Map`'s `MaxConcurrency` ("its default value is 0, which places no
/// limit on concurrency"), a retrier's `MaxAttempts` ("a value of 0 means the error is
/// never retried") and a `Wait`'s `Seconds` — which the language keeps admitting.
const MIN_WINDOW_SECONDS: f64 = 1.0;

/// That bound as the language writes it, which is what the diagnostic about it states
/// ([`minimum_value_message`]): a window is a whole number of seconds, so its bound is
/// stated as the count it is.
const MIN_WINDOW_SECONDS_STATED: &str = "1";

/// The figures a `Map` state declares.
///
/// "MaxConcurrency: [...] Its default value is 0, which places no limit on
/// concurrency", so none is admitted and less than none is not;
/// "ToleratedFailureCount: [...] the number of failed items" is a count, and
/// "ToleratedFailurePercentage: [...] the percentage of failed items" is a percentage,
/// which is bounded at both ends.
///
/// The two counts state their bounds as the whole numbers they are; the percentage is a
/// FRACTIONAL figure, so each end of its range is stated as the decimal the language
/// writes it as — the same reason a retrier's `BackoffRate` states its own bound as
/// [`MIN_BACKOFF_RATE_STATED`].
const MAP_FIGURES: &[Bounded] = &[
    at_least("MaxConcurrency", 0.0, "0"),
    at_least("ToleratedFailureCount", 0.0, "0"),
    Bounded {
        member: "ToleratedFailurePercentage",
        minimum: Some((0.0, "0.0")),
        maximum: Some((100.0, "100.0")),
    },
];

/// The figures a state machine OBJECT declares: "TimeoutSeconds: the maximum number of
/// seconds an execution of the state machine can run", a WINDOW for the same reason a
/// `Task`'s two are, and bounded below at [`MIN_WINDOW_SECONDS`] like them.
const MACHINE_FIGURES: &[Bounded] = &[at_least(
    "TimeoutSeconds",
    MIN_WINDOW_SECONDS,
    MIN_WINDOW_SECONDS_STATED,
)];

/// The figures one `Retry` retrier declares.
///
/// From the language's error-handling rules: "IntervalSeconds [...] represents the
/// number of seconds before the first retry attempt" and "MaxDelaySeconds [...] the
/// maximum amount of time [...] to wait between retry attempts" are each a number of
/// SECONDS; "MaxAttempts [...] the maximum number of retry attempts (default 3). A
/// value of 0 means the error [...] is never retried" is a count, so none is admitted;
/// and "BackoffRate [...] The value must be greater than or equal to 1.0", a rate below
/// which would SHRINK the interval it is documented to grow — the one figure here whose
/// own bound the language states as something other than "not negative".
const RETRIER_FIGURES: &[Bounded] = &[
    at_least("IntervalSeconds", 0.0, "0"),
    at_least("MaxAttempts", 0.0, "0"),
    at_least("MaxDelaySeconds", 0.0, "0"),
    at_least(BACKOFF_RATE_MEMBER, MIN_BACKOFF_RATE, MIN_BACKOFF_RATE_STATED),
];

/// Which figures an object of this kind of state declares, for the one checker below:
/// a type the language does not define declares none, exactly as it declares no fields.
fn bounded_figures(state_type: &str) -> &'static [Bounded] {
    match state_type {
        "Wait" => WAIT_FIGURES,
        "Task" => TASK_FIGURES,
        "Map" => MAP_FIGURES,
        _ => &[],
    }
}

/// The bound rules of ONE object of the definition: each figure it writes that the
/// language bounds and the object wrote outside that bound, reported at the figure
/// itself.
///
/// A figure is judged only where the object DECLARES it (`declared`, the object's own
/// closed field set): a member the object cannot carry is reported as the unsupported
/// field it is rather than twice, and a member whose value is not a number at all
/// carries no figure to bound. Both ends are judged, so one figure can state only the
/// end it broke.
fn diagnose_figures(
    object: &Map<String, Value>,
    figures: &[Bounded],
    declared: &[&str],
    here: &str,
    into: &mut Diagnosis,
) {
    for figure in figures {
        if !declared.contains(&figure.member) {
            continue;
        }
        let Some(written) = object.get(figure.member).and_then(Value::as_f64) else {
            continue;
        };
        let at_figure = at(&nested_at(here, format_args!("/{}", figure.member)), "");
        if let Some((minimum, stated)) = figure.minimum {
            if written < minimum {
                into.push(error(
                    SCHEMA_VALIDATION_FAILED,
                    at_figure.clone(),
                    minimum_value_message(stated),
                ));
                continue;
            }
        }
        if let Some((maximum, stated)) = figure.maximum {
            if written > maximum {
                into.push(error(
                    SCHEMA_VALIDATION_FAILED,
                    at_figure,
                    maximum_value_message(stated),
                ));
            }
        }
    }
}

/// The member a retrier grows its interval by on each attempt.
const BACKOFF_RATE_MEMBER: &str = "BackoffRate";

/// The smallest `BackoffRate` the Amazon States Language admits: "BackoffRate
/// (Optional) … The value must be greater than or equal to 1.0", a rate below which
/// would SHRINK the interval it is documented to grow.
const MIN_BACKOFF_RATE: f64 = 1.0;

/// That bound as the language writes it, which is what the diagnostic about it states
/// ([`minimum_value_message`]): the member carries a rate rather than a count, so its
/// bound is stated as the decimal it is.
const MIN_BACKOFF_RATE_STATED: &str = "1.0";

/// What a schema diagnostic says about a state carrying more than one field of a group
/// the language declares as ALTERNATIVES: the whole group, in brackets, as a list.
///
/// RECORDED text. The group is named in full however many of its fields the state
/// carried, because what the caller has to do is keep ONE of them; WHICH state carries
/// them is what `location` says.
pub fn one_of_fields_message(fields: &[&str]) -> String {
    format!(
        "There should only be one of the following fields: [{}]",
        fields.join(", ")
    )
}

/// The groups of state fields the Amazon States Language declares as ALTERNATIVES, in
/// the order the diagnostic about each group lists them.
///
/// Three kinds of alternative, which are one rule: a value and the `*Path` that READS
/// that value from the state's input ("Error and Cause […] can also be specified using
/// ErrorPath and CausePath", and the same pairing for a `Task`'s two bounds and a
/// `Map`'s items and tolerances); the four ways a `Wait` state declares how long it
/// waits; and the two ways any state declares where the execution goes after it
/// (`Next`, or `"End": true`). A state carrying two fields of one group has declared
/// the same thing twice and the language says which of the two it meant in neither.
///
/// Each group is judged only for a state whose TYPE declares its fields
/// ([`state_fields`]), so a field a type does not declare at all is reported as the
/// unsupported field it is and not as an alternative to something.
///
/// The ORDER inside a group is part of the sentence, and it is the service's own for
/// that group rather than one rule across groups: RECORDED, a `Fail` state's error pair
/// is stated value-first (`[Error, ErrorPath]`) and a `Wait` state's four ways are
/// stated path-first, pair by pair (`[SecondsPath, Seconds, TimestampPath,
/// Timestamp]`). So each group is written here exactly as the diagnostic about THAT
/// group states it, and a group added later takes its own order from the same place.
const EXCLUSIVE_FIELD_GROUPS: &[&[&str]] = &[
    &["Next", "End"],
    &["SecondsPath", "Seconds", "TimestampPath", "Timestamp"],
    &["Error", "ErrorPath"],
    &["Cause", "CausePath"],
    &["TimeoutSeconds", "TimeoutSecondsPath"],
    &["HeartbeatSeconds", "HeartbeatSecondsPath"],
    &["Items", "ItemsPath"],
    &["MaxConcurrency", "MaxConcurrencyPath"],
    &["ToleratedFailureCount", "ToleratedFailureCountPath"],
    &["ToleratedFailurePercentage", "ToleratedFailurePercentagePath"],
];

/// The two names the Amazon States Language declares a `Map` state's processor under,
/// of which a `Map` state carries one: `ItemProcessor`, and the earlier `Iterator`
/// naming the same work — which is why the one the diagnostic names is the first.
const PROCESSOR_FIELDS_OF_A_MAP: &[&str] = &["ItemProcessor", "Iterator"];

/// The fields the Amazon States Language declares on each object a STATE carries beside
/// its own members, in the order its schema states them, paired with the member that
/// carries it.
///
/// Each set is CLOSED like every other object's here, and the source is the language's
/// own schema for the state that carries the object. From "Map state": a
/// `ProcessorConfig` says how the processor runs (`Mode`, and the `ExecutionType` a
/// distributed one runs its children as); an `ItemReader` names the dataset the items
/// are read from (`Resource`, the `ReaderConfig` describing the dataset's own format,
/// and the payload the read is made with under either query language); an `ItemBatcher`
/// says how the items are grouped (`MaxItemsPerBatch`, `MaxInputBytesPerBatch`, each
/// with the `*Path` that reads it, and the `BatchInput` every batch carries); and a
/// `ResultWriter` names where the results are written (`Resource`, its payload, and the
/// `WriterConfig` describing the output). From "Task state": the `Credentials` a task's
/// call is made under, which names the role to assume for it and nothing else.
///
/// A field outside the set of the object it is written in is reported as the
/// unsupported field it is ([`unsupported_field_message`], which suggests the field a
/// misspelling of one of these was meant to be), exactly as a state's own is.
const DECLARED_OBJECT_FIELDS: &[(&str, &[&str])] = &[
    // "Credentials": "Specify a target role ... the Amazon Resource Name (ARN) of the
    // role" — one field, written as the ARN itself or, in JSONPath, as the reference
    // field that reads it from the state's data.
    ("Credentials", &["RoleArn", "RoleArn.$"]),
    ("ProcessorConfig", &["ExecutionType", "Mode"]),
    (
        "ItemReader",
        &["Arguments", "Parameters", "ReaderConfig", "Resource"],
    ),
    (
        "ReaderConfig",
        &[
            "CSVDelimiter",
            "CSVHeaderLocation",
            "CSVHeaders",
            "InputType",
            "MaxItems",
            "MaxItemsPath",
        ],
    ),
    (
        "ItemBatcher",
        &[
            "BatchInput",
            "MaxInputBytesPerBatch",
            "MaxInputBytesPerBatchPath",
            "MaxItemsPerBatch",
            "MaxItemsPerBatchPath",
        ],
    ),
    (
        "ResultWriter",
        &["Arguments", "Parameters", "Resource", "WriterConfig"],
    ),
    ("WriterConfig", &["OutputType", "Transformation"]),
];

/// What a schema diagnostic says about a field whose value the language restricts to a
/// NAMED SET and the definition wrote outside it: the values the field admits.
///
/// RECORDED text, bracketed list and ORDER included. It names no field and does not echo
/// the rejected value — WHICH field broke the rule is what `location` carries, exactly as
/// it is for a figure written outside its bound ([`minimum_value_message`]) — so one
/// sentence serves every field of the schema whose values the language names. The `Type`
/// member is the one such field the service words its own sentence for
/// ([`STATE_TYPE_VALUES_MESSAGE`]).
pub fn field_values_message(values: &[&str]) -> String {
    format!("Value should be one of the following: [{}]", values.join(", "))
}

/// One field of the Amazon States Language whose value the language restricts to a
/// NAMED SET: the field, and the values it admits in the ORDER the service's own
/// sentence about them lists them ([`field_values_message`], which lists the set as it
/// stands here).
///
/// A value outside the set is a schema error of the object that carries it, reported at
/// the field itself ([`diagnose_named_values`]), for the reason a figure outside its
/// bound is ([`Bounded`]): the definition declares work the service cannot do — a
/// column separator that is no separator, an input format nothing reads, a processor
/// mode that is neither of the two the language has — so it is refused where it is
/// submitted rather than failing every execution of the state machine it would have
/// created. A value of any OTHER JSON type is outside the set like any unknown name is,
/// as the `Type` member's own reading records.
struct NamedValues {
    field: &'static str,
    values: &'static [&'static str],
}

/// The fields of the objects a state carries whose values the language names
/// ([`DECLARED_OBJECT_FIELDS`] declares those objects; this declares the sets their
/// fields admit).
///
/// The sets are the language's own, and this emulator's data plane reads each of them
/// in the module that serves it — which is where each entry's source is recorded:
///
/// * a `ReaderConfig` says how a dataset's bytes are items
///   ([`crate::state::item_reader`]): the `InputType` of the dataset, where a CSV
///   dataset's header row comes from (`CSVHeaderLocation`) and which column separator it
///   is written with (`CSVDelimiter`);
/// * a `ProcessorConfig` says how a `Map` state's processor runs — the `Mode` it runs in
///   and, for the distributed one, the `ExecutionType` its child workflow executions run
///   as;
/// * a `WriterConfig` says how the results of such a state are written
///   ([`crate::state::result_writer`]): the `OutputType` of the dataset it writes and the
///   `Transformation` applied to the results before they are written.
///
/// `MANIFEST` is an `InputType` of a dataset this emulator does not READ — an Amazon S3
/// Inventory manifest — and is in the set all the same: what the set says is which values
/// the LANGUAGE admits, so a definition naming one is created and the state that reads it
/// says what was not read ([`crate::state::item_reader`]), exactly as it does for every
/// other dataset it cannot read.
const NAMED_VALUE_FIELDS: &[NamedValues] = &[
    NamedValues {
        field: "InputType",
        values: &["CSV", "JSON", "JSONL", "MANIFEST"],
    },
    NamedValues {
        field: "CSVHeaderLocation",
        values: &["FIRST_ROW", "GIVEN"],
    },
    NamedValues {
        field: "CSVDelimiter",
        // RECORDED order, which is neither the sorted one nor the one the member's
        // documentation lists.
        values: &["COMMA", "SEMICOLON", "PIPE", "SPACE", "TAB"],
    },
    NamedValues {
        field: "Mode",
        values: &["INLINE", DISTRIBUTED_MODE],
    },
    NamedValues {
        field: "ExecutionType",
        values: &[
            crate::state::STATE_MACHINE_TYPE_STANDARD,
            crate::state::STATE_MACHINE_TYPE_EXPRESS,
        ],
    },
    NamedValues {
        field: "OutputType",
        values: &["JSON", "JSONL"],
    },
    NamedValues {
        field: "Transformation",
        values: &["NONE", "COMPACT", "FLATTEN"],
    },
    // A retrier spreads its waits by one of the two strategies the language names:
    // "If you choose FULL ... If you choose NONE" (`super::JITTER_STRATEGY_FULL` is the
    // one this emulator spreads by).
    NamedValues {
        field: "JitterStrategy",
        values: &[super::JITTER_STRATEGY_FULL, "NONE"],
    },
];

/// The values the language admits for the field `field`, or `None` for a field whose
/// value it does not restrict to a named set ([`NAMED_VALUE_FIELDS`]).
fn declared_field_values(field: &str) -> Option<&'static [&'static str]> {
    NAMED_VALUE_FIELDS
        .iter()
        .find(|named| named.field == field)
        .map(|named| named.values)
}

/// The NAMED-VALUE rules of ONE object of the definition: each field it writes that the
/// language restricts to a named set and the object wrote outside that set, reported at
/// the field itself.
///
/// A field is judged only where the object DECLARES it (`declared`, the object's own
/// closed field set), exactly as a bounded figure is: a field the object cannot carry is
/// reported as the unsupported field it is rather than twice.
fn diagnose_named_values(
    object: &Map<String, Value>,
    declared: &[&str],
    here: &str,
    into: &mut Diagnosis,
) {
    for field in into.order().names(here, object) {
        if !declared.contains(&field.as_str()) {
            continue;
        }
        let Some(values) = declared_field_values(field) else {
            continue;
        };
        let Some(written) = object.get(field) else {
            continue;
        };
        if written.as_str().is_some_and(|written| values.contains(&written)) {
            continue;
        }
        into.push(error(
            SCHEMA_VALIDATION_FAILED,
            at(&nested_at(here, format_args!("/{}", echoed(field))), ""),
            field_values_message(values),
        ));
    }
}

/// The fields the language declares on the object `member` carries, or `None` for a
/// member whose value is not such an object ([`DECLARED_OBJECT_FIELDS`]).
fn declared_object_fields(member: &str) -> Option<&'static [&'static str]> {
    DECLARED_OBJECT_FIELDS
        .iter()
        .find(|(name, _)| *name == member)
        .map(|(_, fields)| *fields)
}

/// What a schema diagnostic says about an entry of an `ErrorEquals` that is not an
/// error NAME.
pub const ERROR_NAME_MESSAGE: &str = "'ErrorEquals' must name errors as strings";

/// What the diagnostic for a state machine that cannot stop says.
///
/// RECORDED text. It names no state and no member: what is missing is a terminal
/// state anywhere in the machine, so there is no position in the document to place it
/// at.
pub const MISSING_END_STATE_MESSAGE: &str = "Workflow has no terminal state";

/// What a diagnostic says about a variable a JSONata expression reads and the scope
/// reaching it may not hold: the variable, quoted.
///
/// RECORDED text. The name is the caller's own, so it is clipped by [`echoed`].
pub fn possibly_undefined_variable_message(name: &str) -> String {
    format!("Variable '{}' is possibly not defined.", echoed(name))
}

/// What a diagnostic says about a variable a field the language RESOLVES a path at
/// names and the scope reaching that field may not hold: the REFERENCE as the field
/// wrote it (`$name`, which is how the JSONPath half of the language spells a variable
/// read), quoted, and that it is not defined.
///
/// RECORDED text, and the counterpart of [`possibly_undefined_variable_message`]: that
/// one is about a read a run may or may not resolve, and this one about a path there is
/// nothing to resolve against.
pub fn undefined_variable_message(name: &str) -> String {
    format!("The variable '${}' is not defined.", echoed(name))
}

/// What a diagnostic says about an `Assign` block whose KEY is not a name a state could
/// assign a variable under ([`super::is_variable_name`]).
///
/// RECORDED text: the service states the KEY it could not read as a variable name — the
/// one value in the sentence — and then the rule that name broke. ONE sentence, so the
/// name a `TestState` request supplies and the name a definition assigns are refused in
/// the same words.
pub fn invalid_variable_name_message(name: &str) -> String {
    format!(
        "Invalid variable name '{}': the variable name contains invalid characters.",
        echoed(name)
    )
}

/// Everything ONE walk of a definition found: the diagnostics it reports, and
/// whether it had to drop any of them.
///
/// The walk collects at most [`MAX_DIAGNOSTICS`] of each severity, so a caller is
/// told that more exist (`truncated`) rather than being handed a list that grows
/// with the definition.
#[derive(Debug, Clone)]
pub struct Diagnosis {
    /// What was found, in the order it was found.
    pub diagnostics: Vec<Diagnostic>,
    /// Whether an ERROR was found and not reported, the error budget being spent.
    pub errors_dropped: bool,
    /// Whether a WARNING was found and not reported, that budget being spent.
    pub warnings_dropped: bool,
    /// How many of each severity have been reported, which is what spends each
    /// budget.
    errors: usize,
    warnings: usize,
    /// Whether this walk reports no WARNING at all — what reads it ([`validate`],
    /// [`validate_single_state`]) refuses a definition for its ERRORS, so a warning
    /// would be built only to be discarded.
    errors_only: bool,
    /// Whether this walk reads the FIRST error and nothing else, so there is no reason
    /// to keep walking past that error.
    first_error_only: bool,
    /// What is LEFT of this diagnosis's variable-scope budget
    /// ([`MAX_SCOPE_VISITS`]), spent by every state that analysis visits, in this
    /// definition and in every machine nested in it.
    scope_visits: usize,
    /// What is LEFT of this diagnosis's expression-PARSE budget
    /// ([`MAX_PARSED_EXPRESSIONS`]), spent by every field of a JSONata state whose
    /// value is one whole expression, in this definition and in every machine nested
    /// in it.
    parses: usize,
    /// The order the DEFINITION writes its members in, which is the order the problems
    /// found in them are reported in ([`MemberOrder`]): the walk reads every object of
    /// the definition through it, so what it reports follows the submitted document
    /// rather than the parse's sorting of the names.
    order: MemberOrder,
    /// The WORKFLOW TYPE the definition is being judged as, which the rules that depend
    /// on it read ([`Self::express`]): the Amazon States Language is one language for
    /// both types, and what differs is the work a state machine of each type can
    /// perform. `CreateStateMachine`'s own `type` (defaulting to
    /// [`crate::state::STATE_MACHINE_TYPE_STANDARD`], as that member documents) is what
    /// decides it, so a walk told nothing judges a definition as the default type.
    workflow_type: &'static str,
    /// The variables already in scope for the definition being judged, which no state
    /// of it assigns: `TestState`'s `variables` are "variables set up as part of the
    /// test", so a state under test reads them exactly as it reads what a state before
    /// it assigned. A walk told of none judges every read against the definition alone.
    given: BTreeSet<String>,
}

/// A walk with every budget of it unspent.
impl Default for Diagnosis {
    fn default() -> Self {
        Self {
            diagnostics: Vec::new(),
            errors_dropped: false,
            warnings_dropped: false,
            errors: 0,
            warnings: 0,
            errors_only: false,
            first_error_only: false,
            scope_visits: MAX_SCOPE_VISITS,
            parses: MAX_PARSED_EXPRESSIONS,
            order: MemberOrder::none(),
            workflow_type: crate::state::STATE_MACHINE_TYPE_STANDARD,
            given: BTreeSet::new(),
        }
    }
}

impl Diagnosis {
    /// A walk that stops at the first ERROR, for [`validate_single_state`]: no warning
    /// is built and nothing past that error is looked at. What it reports about a
    /// definition is the same verdict a full walk reports, because an error is an error
    /// wherever in the definition it was found.
    pub(super) fn until_first_error() -> Self {
        Self {
            errors_only: true,
            first_error_only: true,
            ..Self::default()
        }
    }

    /// A walk that reports every ERROR and no warning, for [`validate`]: the refusal it
    /// words states one subject per error the definition carries, and a warning is not
    /// one of them.
    pub(super) fn errors_only() -> Self {
        Self {
            errors_only: true,
            ..Self::default()
        }
    }

    /// Whether the definition carries a problem that prevents creating or updating
    /// a state machine with it — what `result` reports as `FAIL`.
    pub fn failed(&self) -> bool {
        self.diagnostics.iter().any(Diagnostic::is_error)
    }

    /// Whether a diagnostic of a severity this request reports was dropped, so the
    /// reported list is short of what the definition carries.
    pub fn dropped(&self, severity: &str) -> bool {
        self.errors_dropped
            || (severity == crate::state::SEVERITY_WARNING && self.warnings_dropped)
    }

    /// Record one diagnostic, or that its severity's budget is spent.
    pub(super) fn push(&mut self, found: Diagnostic) {
        let (reported, dropped) = if found.is_error() {
            (&mut self.errors, &mut self.errors_dropped)
        } else {
            (&mut self.warnings, &mut self.warnings_dropped)
        };
        if *reported >= MAX_DIAGNOSTICS || (self.errors_only && !found.is_error()) {
            *dropped = true;
            return;
        }
        *reported += 1;
        self.diagnostics.push(found);
    }

    /// This walk, reading every object of the definition in the order `order` says
    /// that definition writes its members in.
    pub(super) fn ordered_by(mut self, order: MemberOrder) -> Self {
        self.order = order;
        self
    }

    /// That order, for the walk to read one object's members through.
    pub(super) fn order(&self) -> &MemberOrder {
        &self.order
    }

    /// This walk, judging the definition as a state machine of the workflow type
    /// `workflow_type` — [`crate::state::STATE_MACHINE_TYPE_STANDARD`] or
    /// [`crate::state::STATE_MACHINE_TYPE_EXPRESS`].
    pub(super) fn for_workflow_type(mut self, workflow_type: &'static str) -> Self {
        self.workflow_type = workflow_type;
        self
    }

    /// Whether the definition is being judged as an EXPRESS state machine, which is
    /// what the rules about the work each workflow type can perform read.
    pub(super) fn express(&self) -> bool {
        self.workflow_type == crate::state::STATE_MACHINE_TYPE_EXPRESS
    }

    /// This walk, with `given` already in scope for every state it judges.
    pub(super) fn with_variables(mut self, given: BTreeSet<String>) -> Self {
        self.given = given;
        self
    }

    /// Whether `name` is one of the variables the REQUEST supplied, which a read of it
    /// resolves against however the definition assigns it.
    pub(super) fn given_variable(&self, name: &str) -> bool {
        self.given.contains(name)
    }

    /// Whether this walk reports WARNINGS at all: the lints that cost a walk of their
    /// own are built only where one can be reported.
    pub(super) fn warnings_wanted(&self) -> bool {
        !self.errors_only
    }

    /// Spend `visits` states of the variable-scope budget, or report that what is left
    /// does not cover them.
    ///
    /// The budget is the ONE this diagnosis has for every machine it walks, so the
    /// analysis's cost cannot be the product of the machines a definition declares and
    /// the states each of them holds.
    pub(super) fn spend_scope_visits(&mut self, visits: usize) -> bool {
        match self.scope_visits.checked_sub(visits) {
            Some(left) => {
                self.scope_visits = left;
                true
            }
            None => false,
        }
    }

    /// Spend ONE expression of the parse budget, or report that it is spent.
    ///
    /// The budget is the ONE this diagnosis has for every machine it walks: a parse is
    /// recursive descent over a caller's text on a stack sized for it
    /// ([`super::super::jsonata::parse_diagnostic`]), so how many of them one request
    /// can ask for is fixed here rather than by the number of expressions a
    /// caller-sized definition declares. It is the per-severity diagnostic budget
    /// ([`MAX_DIAGNOSTICS`]), so a walk can still report every parse failure it is
    /// allowed to report.
    pub(super) fn spend_parse(&mut self) -> bool {
        match self.parses.checked_sub(1) {
            Some(left) => {
                self.parses = left;
                true
            }
            None => false,
        }
    }

    /// Whether the walk keeps looking: it stops once it can report no further ERROR
    /// — its error budget spent, or, for a [`validate_single_state`], the one error it
    /// reads already found. Every error past that point would change neither the result
    /// nor the reported list.
    pub(super) fn looking(&self) -> bool {
        !(self.errors_dropped || (self.first_error_only && self.errors > 0))
    }
}

/// What separates the subjects of an `InvalidDefinition` message: a definition
/// carrying several problems is refused with one subject per problem, written one
/// after another in the order the walk found them.
pub(super) const SUBJECT_SEPARATOR: &str = ", ";

/// Why `definition` is not a definition this service accepts, or `Ok` when it is.
///
/// These are the Amazon States Language's own structural rules, so the refusal is
/// `CreateStateMachine`'s modelled `InvalidDefinition`: the definition is a JSON
/// object carrying `StartAt` and a non-empty `States` map, `StartAt` names a declared
/// state, every state is an object with a string `Type`, every `Next` names a
/// declared state, a terminal state carries no `Next`, and a continuing state carries
/// `Next` or `"End": true`.
///
/// The `Err` is the SUBJECT of that error's message, which the operation states
/// through [`crate::state::resource_error_message`] with
/// [`crate::state::INVALID_STATE_MACHINE_DEFINITION`]: ONE subject per error
/// [`diagnose`] found, each `<code>: <detail> at <location>` ([`Diagnostic::stated`]),
/// separated by [`SUBJECT_SEPARATOR`]. Every error the definition carries is stated,
/// so a caller fixing a definition is told all of them rather than one per attempt,
/// and what a create refuses a definition for cannot disagree with what
/// `ValidateStateMachineDefinition` reports `FAIL` for.
///
/// The subjects are stated in the order the members they are about are written in the
/// SUBMITTED definition, which is the order the walk reads every object of it in
/// ([`MemberOrder`]) — not sorted by location, which is what the parse's own ordering
/// of an object's names would state them in.
///
/// The message is bounded by the walk itself rather than by the definition: the walk
/// reports at most [`MAX_DIAGNOSTICS`] errors, each one's quoted value is capped by
/// [`MAX_ECHOED_CHARS`] and its location by [`MAX_LOCATION_CHARS`], so a caller-sized
/// definition cannot be amplified into a larger refusal.
///
/// A definition may use states this emulator does not EXECUTE and still be valid,
/// because it is valid ASL: what cannot be run surfaces at execution time, as a
/// `FAILED` execution, not as a rejected create.
pub fn validate(definition: &str) -> Result<(), String> {
    validate_as(definition, crate::state::STATE_MACHINE_TYPE_STANDARD)
}

/// [`validate`] for a definition being created as a state machine of the workflow type
/// `workflow_type`.
///
/// The Amazon States Language is ONE language for both workflow types, and what differs
/// is the work a state machine of each type can perform: an integration pattern only
/// the STANDARD type serves ([`diagnose_task_resource`]) is a defect of the definition
/// where the type being created cannot perform it, so the type is part of what the
/// definition is judged against. Every other rule reads it not at all, which is why
/// [`validate`] — the reader for a definition whose type is the default — is the same
/// walk.
pub fn validate_as(definition: &str, workflow_type: &'static str) -> Result<(), String> {
    validate_with(definition, workflow_type, BTreeSet::new())
}

/// [`validate_as`] with `given` already in scope for every state of the definition:
/// `TestState`'s `variables` are supplied with the request, so a state machine
/// definition it tests one state OF reads them like any other variable in scope
/// ([`validate_single_state_with`] is the same reading for a lone state).
pub fn validate_with(
    definition: &str,
    workflow_type: &'static str,
    given: BTreeSet<String>,
) -> Result<(), String> {
    let stated: Vec<String> = collect(
        definition,
        Diagnosis::errors_only()
            .for_workflow_type(workflow_type)
            .with_variables(given),
    )
    .diagnostics
    .iter()
    .filter(|found| found.is_error())
    .map(Diagnostic::stated)
    .collect();
    if stated.is_empty() {
        return Ok(());
    }
    Err(stated.join(SUBJECT_SEPARATOR))
}

/// Every problem `definition` carries: the errors that would prevent creating or
/// updating a state machine with it, and the warnings that would not.
///
/// This is what `ValidateStateMachineDefinition` reports — "Validation will look
/// for problems in your state machine definition and return a result and a list of
/// diagnostic elements" — and, through [`validate`], what `CreateStateMachine` and
/// `UpdateStateMachine` refuse a definition for.
///
/// The rules are applied to EVERY state list the definition declares, not only the
/// top-level one: a `Map` state's `ItemProcessor`/`Iterator` and a `Parallel` state's
/// `Branches` each declare their own state machine, whose state names reach the same
/// execution history and whose states inherit the enclosing query language.
///
/// The list is bounded by [`MAX_DIAGNOSTICS`] per severity; a definition carrying
/// more problems than that is reported as truncated rather than reported in full.
///
/// The walk costs the same ORDER as the parse that precedes it: it visits each
/// member once, every path it builds is bounded by [`MAX_LOCATION_CHARS`] and every
/// value it quotes by [`MAX_ECHOED_CHARS`], so no cost that is the product of the
/// state count and the nesting can be built from a caller's definition.
pub fn diagnose(definition: &str) -> Diagnosis {
    diagnose_as(definition, crate::state::STATE_MACHINE_TYPE_STANDARD)
}

/// [`diagnose`] for a definition judged as a state machine of the workflow type
/// `workflow_type`, which `ValidateStateMachineDefinition`'s own `type` member names
/// ("The target type of state machine for this definition") and which
/// [`validate_as`] judges a create against.
pub fn diagnose_as(definition: &str, workflow_type: &'static str) -> Diagnosis {
    collect(
        definition,
        Diagnosis::default().for_workflow_type(workflow_type),
    )
}

/// [`diagnose`] into the walk `found` describes: every problem, only the errors — or,
/// for [`super::validate_single_state`], the first error and nothing else.
pub(super) fn collect(definition: &str, mut found: Diagnosis) -> Diagnosis {
    if definition.trim().is_empty() {
        found.push(error(
            MISSING_DESCRIPTION,
            None,
            "the request carried no definition to validate",
        ));
        return found;
    }
    // What is found in the definition is reported in the order the definition writes
    // the members it is found in, so that order is read off the submitted document
    // before anything is judged.
    let mut found = found.ordered_by(MemberOrder::of(definition));
    match serde_json::from_str::<Value>(definition) {
        Ok(document) => diagnose_machine(
            &document,
            "",
            JSONPATH,
            1,
            false,
            Outer::Nothing,
            &mut found,
        ),
        // The diagnostic is the service's own, naming where the definition stopped
        // parsing, the mark it left unmatched and what was expected there — the one
        // construction every document this service reads is reported through — and
        // it is LOCATED at the position in the definition the parse ended at, which
        // is where a client has to look.
        Err(why) => found.push(error(
            INVALID_JSON_DESCRIPTION,
            Some(crate::state::json_parse_definition_location(definition, &why)),
            crate::state::json_parse_definition_diagnostic(definition, &why),
        )),
    }
    found
}

/// [`diagnose`] for one state machine — the definition itself, or one nested inside
/// it — carrying where it was found (`prefix`), the query language it inherits from
/// the state that declares it, how deeply it is nested, and whether it is a `Map`
/// state's PROCESSOR, which admits one field more than a definition or a `Parallel`
/// branch does ([`machine_fields`]), and what was already in scope where it is
/// declared ([`Outer`]).
pub(super) fn diagnose_machine(
    document: &Value,
    prefix: &str,
    inherited: &str,
    depth: usize,
    processor: bool,
    outer: Outer<'_>,
    into: &mut Diagnosis,
) {
    if !into.looking() {
        return;
    }
    if depth > MAX_DEFINITION_DEPTH {
        into.push(error(
            TOO_DEEPLY_NESTED,
            machine_at(prefix),
            format!(
                "the definition nests state machines more than the {MAX_DEFINITION_DEPTH} levels \
                 this service reads"
            ),
        ));
        return;
    }
    let Some(document) = document.as_object() else {
        into.push(error(
            SCHEMA_VALIDATION_FAILED,
            machine_at(prefix),
            "a state machine definition must be a JSON object",
        ));
        return;
    };
    // `QueryLanguage` selects which members the states below are written with, so
    // it is read before them; a definition that names neither language is written
    // in the one `QueryLanguage` defaults to.
    let language = match declared_language(document.get("QueryLanguage")) {
        Ok(declared) => declared.unwrap_or(inherited),
        Err(why) => {
            into.push(error(
                SCHEMA_VALIDATION_FAILED,
                at(prefix, "/QueryLanguage"),
                why,
            ));
            inherited
        }
    };
    // The language's schema for a state machine object is CLOSED: it declares the
    // fields a definition may carry ([`MACHINE_FIELDS`]) and the two it must
    // ([`MACHINE_REQUIRED_FIELDS`]). Fields the object does not carry are reported
    // TOGETHER, in one diagnostic that lists them, and each field the schema does
    // not declare is reported on its own — both located at the OBJECT, which is
    // what a field belongs to, rather than at a position the document has not got.
    let object = machine_at(prefix);
    let missing: Vec<&str> = MACHINE_REQUIRED_FIELDS
        .iter()
        .copied()
        .filter(|field| !document.contains_key(*field))
        .collect();
    if !missing.is_empty() {
        into.push(error(
            SCHEMA_VALIDATION_FAILED,
            object.clone(),
            required_fields_message(&missing),
        ));
    }
    for field in into.order().names(prefix, document) {
        if !machine_fields(processor).contains(&field.as_str()) {
            into.push(error(
                SCHEMA_VALIDATION_FAILED,
                object.clone(),
                unsupported_field_message(field, machine_fields(processor)),
            ));
        }
    }
    let states = document.get("States").and_then(Value::as_object);
    // The figures a state machine OBJECT writes that the language bounds, judged by the
    // one checker every bounded figure of a definition is judged by.
    diagnose_figures(
        document,
        MACHINE_FIGURES,
        machine_fields(processor),
        prefix,
        into,
    );
    // `Version` names the version of the Amazon States Language the object is written
    // in, and the language defines ONE ([`LANGUAGE_VERSIONS`]): a value outside that set
    // names a language this service does not implement, so the object declares semantics
    // nothing could run it under. Judged where the object DECLARES the member, like every
    // figure above, and reported at the member itself.
    if let Some(declared) = document.get("Version") {
        if !declared
            .as_str()
            .is_some_and(|named| LANGUAGE_VERSIONS.contains(&named))
        {
            into.push(error(
                SCHEMA_VALIDATION_FAILED,
                at(prefix, "/Version"),
                one_of_values_message(LANGUAGE_VERSIONS),
            ));
        }
    }
    // And the OBJECTS it carries whose own fields the language declares — a `Map`
    // state's processor carries the `ProcessorConfig` that says how it runs.
    diagnose_declared_objects(document, machine_fields(processor), prefix, into);
    // A field the object DOES carry is judged on its own: that it is absent is
    // already stated above, so what is left to say about it is its value.
    let mut start_at: Option<&str> = None;
    if let Some(declared) = document.get("StartAt") {
        match declared.as_str() {
            Some(declared) => {
                start_at = Some(declared);
                if states.is_some_and(|states| !states.contains_key(declared)) {
                    into.push(error(
                        MISSING_TRANSITION_TARGET,
                        at(prefix, "/StartAt"),
                        missing_target_message(declared),
                    ));
                }
            }
            None => into.push(error(
                SCHEMA_VALIDATION_FAILED,
                at(prefix, "/StartAt"),
                "a state machine definition must carry a string 'StartAt' member",
            )),
        }
    }
    let Some(states) = states else {
        if document.contains_key("States") {
            into.push(error(
                SCHEMA_VALIDATION_FAILED,
                at(prefix, "/States"),
                "a state machine definition must carry an object 'States' member",
            ));
        }
        return;
    };
    if states.is_empty() {
        into.push(error(
            SCHEMA_VALIDATION_FAILED,
            at(prefix, "/States"),
            EMPTY_VALUE_MESSAGE,
        ));
        return;
    }
    // What the definition's transitions NAME: the state its `StartAt` names, and
    // every state a transition of a declared state names. Read once per machine,
    // before any state is judged, because it is a property of the whole graph rather
    // than of one state.
    let named = named_states(states, start_at);
    // Which variables each state of THIS machine can count on having been assigned
    // by the time it runs, and which ones it assigns at all, which is what its own
    // reads are judged against. Built once per machine, before any state is judged,
    // because it is a property of the whole graph rather than of one state: a read
    // nothing in scope assigns is an ERROR, so every walk needs it and not only one
    // reporting WARNINGS.
    let scopes = Scopes::of(states, start_at, outer, into);
    // Whether the machine declares a state that ENDS an execution, read before the
    // states are judged and reported after them: the diagnostic is about the machine,
    // so it follows the ones about its states.
    let terminal = states.values().any(ends_an_execution);
    // The states are judged in the order the definition DECLARES them, so what is
    // reported about one state follows what is reported about the state written before
    // it.
    let declared_states = nested_at(prefix, "/States");
    for (name, state) in into.order().members(&declared_states, states) {
        if !into.looking() {
            break;
        }
        diagnose_state(
            name,
            state,
            &Machine {
                states,
                language,
                prefix,
                depth,
            },
            &scopes,
            into,
        );
    }
    // The transition graph is also read the other way round: a state nothing in the
    // definition names is work this state machine can never do, so it is reported
    // under the code the transition graph's problems are reported under, at the state
    // itself — what is wrong is not a member the state carries but that nothing leads
    // to it. These follow the diagnostics about the states, because they are about
    // the graph those states form.
    //
    // A machine that declares no entry point at all has no graph to judge this way:
    // that it carries no `StartAt` is already reported above, and every state of it
    // would be unreached for that one reason.
    for name in into.order().names(&declared_states, states) {
        if !into.looking() {
            break;
        }
        if start_at.is_none() || named.contains(name.as_str()) {
            continue;
        }
        into.push(error(
            MISSING_TRANSITION_TARGET,
            at(&state_pointer(prefix, name), ""),
            unreachable_state_message(name),
        ));
    }
    // "MISSING_END_STATE: The workflow does not have a terminal state." A state
    // machine is a graph that must be able to STOP, so at least one of the states it
    // declares ends an execution; one that declares none describes a workflow no run
    // of it can leave. It is read BEFORE the states are judged and reported AFTER
    // them, so it follows the diagnostics about them; whether there is room left to
    // report it is `push`'s own answer, as it is for every other diagnostic.
    //
    // What is missing is a terminal state ANYWHERE in the workflow this machine
    // describes, so the diagnostic sits at no position in the document and carries no
    // location — for a `Parallel` branch and a `Map` state's processor, each its own
    // scope, exactly as for the definition itself ([`Diagnostic::stated`] is what a
    // refusal states an absent location as).
    if !terminal {
        into.push(error(MISSING_END_STATE, None, MISSING_END_STATE_MESSAGE));
    }
}

/// What every state of ONE state machine is judged in: the state list its transition
/// targets resolve against, the query language it inherits from the machine, where
/// that machine was found and how deeply it is nested.
struct Machine<'a> {
    states: &'a Map<String, Value>,
    language: &'a str,
    prefix: &'a str,
    depth: usize,
}

/// The states of `states` that something NAMES: the one `start_at` names, and every
/// one a transition of a declared state names ([`transitions`]).
///
/// A state outside this set is one nothing in the definition leads to, which is what
/// [`unreachable_state_message`] reports. A transition naming a state the machine does
/// not declare names nothing here and is reported on its own
/// ([`MISSING_TRANSITION_TARGET`]); an entry point that names no declared state leaves
/// the machine with nothing named but what its states name.
///
/// Each state's transitions are read once, so this costs one pass over the machine
/// whatever shape its graph has.
fn named_states<'a>(
    states: &'a Map<String, Value>,
    start_at: Option<&str>,
) -> BTreeSet<&'a str> {
    // The name is taken from the `States` map rather than from what named it, so the
    // set holds the machine's own text once however many transitions name a state.
    let declared = |name: &str| {
        states
            .get_key_value(name)
            .map(|(declared, _)| declared.as_str())
    };
    let mut named: BTreeSet<&str> = start_at.and_then(declared).into_iter().collect();
    for state in states.values() {
        let Some(state) = state.as_object() else {
            continue;
        };
        for transition in transitions(state) {
            named.extend(declared(transition.target.as_str()));
        }
    }
    named
}

/// Whether `state` ENDS an execution: a state of a type that terminates one
/// ([`TERMINAL_STATE_TYPES`]), or any state declaring `"End": true`.
///
/// This is what [`MISSING_END_STATE`] is read off, over every state a machine
/// DECLARES: what that diagnostic reports is that the workflow has no terminal state,
/// and a state machine that declares one has one — whether anything this definition
/// declares LEADS to it is a different question, and the one [`named_states`] answers.
fn ends_an_execution(state: &Value) -> bool {
    let Some(state) = state.as_object() else {
        return false;
    };
    if state.get("End").and_then(Value::as_bool) == Some(true) {
        return true;
    }
    state
        .get("Type")
        .and_then(Value::as_str)
        .is_some_and(|state_type| TERMINAL_STATE_TYPES.contains(&state_type))
}

/// [`diagnose_machine`] for ONE state of a state machine: the rules the state's own
/// members must satisfy, then the state machines nested inside it.
///
/// `scopes` is its machine's variable-scope analysis, which answers what every run
/// reaching this state has already assigned — what its own reads are judged against.
fn diagnose_state<'a>(
    name: &'a str,
    state: &Value,
    machine: &Machine<'_>,
    scopes: &'a Scopes<'a>,
    into: &mut Diagnosis,
) {
    let Machine {
        states,
        language: inherited,
        prefix,
        depth,
    } = *machine;
    // The name the state is DECLARED under, which is what its machine's analysis
    // answers for.
    let declared = name;
    // A state name is a caller-chosen JSON object key and lands in every history
    // event of that state, so its LENGTH is bounded here, not only the number of
    // events: this is what keeps a history's bytes bounded. The length checked and
    // reported is the DECLARED one.
    let declared_chars = name.chars().count();
    // WHERE this state is: the pointer every diagnostic about it is located at, and
    // the prefix the machines nested in it are judged under. It names the state as the
    // DEFINITION declares it ([`state_pointer`]) — a pointer carrying a shortened
    // rendering of the name would resolve to no state of the submitted document, which
    // is what a client reads it to find.
    let here = state_pointer(prefix, declared);
    // The name a sentence about one of the state's OTHER rules quotes is the clipped
    // one, for the reason [`echoed`] records: it is a caller value quoted inside a
    // sentence about something else.
    let shown = echoed(name);
    let name = shown.as_str();
    if declared_chars > MAX_STATE_NAME_CHARS {
        into.push(error(
            INVALID_STATE_NAME,
            at(&here, ""),
            state_name_too_long_message(declared),
        ));
    }
    let Some(state) = state.as_object() else {
        into.push(error(
            SCHEMA_VALIDATION_FAILED,
            at(&here, ""),
            format!("state '{name}' must be a JSON object"),
        ));
        return;
    };
    // `Type` is the field EVERY state owes, whatever work it does, so a state that
    // does not carry it is reported the way every object's missing fields are: the
    // field it owes, located at the state ([`required_fields_message`]).
    let Some(declared_type) = state.get("Type") else {
        into.push(error(
            SCHEMA_VALIDATION_FAILED,
            at(&here, ""),
            required_fields_message(&[STATE_TYPE_MEMBER]),
        ));
        return;
    };
    // `Type` selects one of the state types the Amazon States Language DEFINES
    // ([`STATE_TYPES`]), so a state whose `Type` is not one of them describes no state
    // at all: nothing says what it reads, what it writes or where it goes next, which
    // is why the walk stops here rather than judging the state's other members under
    // rules that are keyed on its type. A value of any OTHER JSON type is outside that
    // set like any unknown name is, and is reported as the member it is written at.
    let Some(state_type) = declared_type
        .as_str()
        .filter(|declared| STATE_TYPES.contains(declared))
    else {
        into.push(error(
            SCHEMA_VALIDATION_FAILED,
            at(&here, &format!("/{STATE_TYPE_MEMBER}")),
            STATE_TYPE_VALUES_MESSAGE,
        ));
        return;
    };
    // The language's schema for a state says which fields the type it names OWES
    // ([`state_required_fields`]) as well as which it may carry: a state satisfying
    // none of a requirement declares no work for the thing its type does, so it is
    // reported exactly as a state machine object's missing fields are — stated as the
    // requirement it broke ([`missing_fields_message`]) and located at the STATE,
    // which is what owes it.
    for alternatives in missing_state_fields(state_type, |field| state.contains_key(field)) {
        into.push(error(
            SCHEMA_VALIDATION_FAILED,
            at(&here, ""),
            missing_fields_message(alternatives),
        ));
    }
    // The language's schema for a state is CLOSED per TYPE, exactly as a state
    // machine object's is: the type selects what the state does, and the fields
    // beside it say how ([`state_fields`]), so each field outside that set is
    // reported on its own, located at the STATE — the field belongs to the state
    // rather than to a position under it.
    for field in into.order().names(&here, state) {
        if !state_fields(state_type).contains(&field.as_str()) {
            into.push(error(
                SCHEMA_VALIDATION_FAILED,
                at(&here, ""),
                unsupported_field_message(field, state_fields(state_type)),
            ));
        }
    }
    // What the state does with a failure of its own work, for a type that declares
    // it.
    diagnose_error_handlers(state, state_type, &here, into);
    // The figures the state writes that the language BOUNDS, each judged where its
    // own type declares it ([`bounded_figures`]).
    diagnose_figures(
        state,
        bounded_figures(state_type),
        state_fields(state_type),
        &here,
        into,
    );
    // What a `Task` state's work IS, judged against the workflow type the definition
    // is being created as: an integration PATTERN this workflow type cannot perform is
    // work no execution of it could ever do.
    diagnose_task_resource(state, state_type, &here, into);
    // The rules a `Choice` state's own rules must satisfy: each is an object whose
    // fields the language declares, and each of the state's OWN rules says where the
    // execution goes when it is the rule that holds.
    if state_type == "Choice" {
        diagnose_choice_rules(
            state.get("Choices"),
            &here,
            "Choices",
            true,
            MAX_EXPRESSION_DEPTH,
            into,
        );
    }
    // And the OBJECTS a state carries beside its own members, whose fields the language
    // declares as closely as it declares a state's ([`DECLARED_OBJECT_FIELDS`]): a field
    // outside one of those sets is reported as the unsupported field it is, located at
    // the object that carries it.
    diagnose_declared_objects(state, state_fields(state_type), &here, into);
    // The fields the language declares as ALTERNATIVES, of which a state may carry
    // one.
    diagnose_exclusive_fields(state, state_type, &here, into);
    // A `Fail` state's `Error` and `Cause` are reported verbatim by
    // `DescribeExecution`, through shapes with their own maxima, so a
    // definition carrying a longer one could never be reported.
    for (member, cap) in [("Error", MAX_ERROR_BYTES), ("Cause", MAX_CAUSE_BYTES)] {
        if let Some(value) = state.get(member).and_then(Value::as_str) {
            if value.len() > cap {
                into.push(error(
                    SCHEMA_VALIDATION_FAILED,
                    at(&here, &format!("/{member}")),
                    format!(
                        "state '{name}' carries a '{member}' of {} bytes, more than the {cap} \
                         this service reports",
                        value.len()
                    ),
                ));
            }
        }
    }
    // Every member the state names a transition through — its own `Next`, the
    // `Default` of a `Choice`, each `Choice` rule's own `Next` and each `Catch`
    // catcher's `Next` — resolves by ONE rule: the name must be one the enclosing
    // `States` map declares. Each is reported at the member that names it.
    for transition in transitions(state) {
        if !states.contains_key(transition.target.as_str()) {
            into.push(error(
                MISSING_TRANSITION_TARGET,
                at(&here, &transition.path),
                missing_target_message(&transition.target),
            ));
        }
    }
    // A type that ENDS an execution declares no `Next` at all, so a state of one
    // carrying the member is reported ONCE, by the closed field set every state is
    // judged against above ([`state_fields`]): the field is not one the language
    // declares there, which is the whole of what is wrong with it.
    match state.get("Next") {
        Some(next) => {
            if next.as_str().is_none() {
                into.push(error(
                    SCHEMA_VALIDATION_FAILED,
                    at(&here, "/Next"),
                    format!("state '{name}' has a non-string 'Next' member"),
                ));
            }
        }
        None => {
            if CONTINUING_STATE_TYPES.contains(&state_type)
                && state.get("End").and_then(Value::as_bool) != Some(true)
            {
                into.push(error(
                    SCHEMA_VALIDATION_FAILED,
                    at(&here, ""),
                    format!(
                        "state '{name}' is a '{}' state, so it must carry either \
                         'Next' or \"End\": true",
                        echoed(state_type)
                    ),
                ));
            }
        }
    }
    // A state may be written in a query language of its own — but only in the
    // direction the service allows: a JSONPath state machine may hold a JSONata
    // state, a JSONata one may not hold a JSONPath state.
    let language = match declared_language(state.get("QueryLanguage")) {
        Ok(Some(declared)) => {
            if declared == JSONPATH && inherited == JSONATA {
                // The service blames the overriding STATE rather than the member it
                // declares the override in: the state is the thing a state machine of
                // this query language may not hold. Which state that is, is reported
                // by `location` alone — the sentence is about the member and is the
                // same for every state that carries it.
                into.push(error(
                    SCHEMA_VALIDATION_FAILED,
                    at(&here, ""),
                    format!(
                        "'QueryLanguage' can not be '{JSONPATH}' if set to '{JSONATA}' \
                         for whole state machine"
                    ),
                ));
            }
            declared
        }
        Ok(None) => inherited,
        Err(why) => {
            into.push(error(
                SCHEMA_VALIDATION_FAILED,
                at(&here, "/QueryLanguage"),
                why,
            ));
            inherited
        }
    };
    if language == JSONATA {
        diagnose_jsonata_state(name, state, state_type, &here, into);
    } else {
        diagnose_jsonpath_state(name, state, state_type, &here, into);
    }
    // A variable the state ASSIGNS is one the definition declares AT this state, so a
    // read of it here names nothing the definition leaves undeclared: a field that reads
    // it before the assignment that writes it has been made fails when the state RUNS
    // rather than when it is created, which is what the warning below reports.
    let own = assigned_variables(state);
    // The ASSIGNMENTS themselves: a block of them is an object, and each name it
    // assigns is a name the language admits for a variable.
    diagnose_assignments(state, state_type, &here, into);
    // One diagnostic per FIELD that reads a variable the scope reaching it may not
    // hold, which is the granularity `location` documents
    // (`/States/<StateName>/<FieldName>`), reported in the order the state writes those
    // fields — of the code and the severity WHAT COULD HAVE ASSIGNED IT decides:
    //
    // * a name NO run reaching the field could have assigned is a read with nothing to
    //   resolve against on any run, so a field the language RESOLVES a path at
    //   ([`reads_a_path`]: the `*Path` family, a payload template's reference field, a
    //   `Choice` rule's `Variable`) is an ERROR ([`UNDEFINED_VARIABLE`]): the definition
    //   declares no assignment that could reach it;
    // * a name SOME run reaching the field has assigned and another has not resolves on
    //   some runs and not on others — a `Catch` catcher's `Assign` seen by a state the
    //   success path also reaches is exactly that — so the definition is CREATED and the
    //   read earns the warning ([`POSSIBLY_UNDEFINED_VARIABLE`]), whichever language it
    //   is written in;
    // * every read written as a JSONata expression is that warning too, whatever the
    //   scope: the expression EVALUATES to nothing where the scope does not hold the
    //   name and fails the state that reads it then (`States.QueryEvaluationError`).
    let reads = variable_reads(state, &here, into.order());
    for (field, (read, reference_field)) in reads {
        if own.contains(&read) || scopes.holds(declared, &read) || into.given_variable(&read) {
            continue;
        }
        if reference_field && !scopes.may_hold(declared, &read) {
            into.push(error(
                UNDEFINED_VARIABLE,
                at(&here, &field),
                undefined_variable_message(&read),
            ));
            continue;
        }
        if into.warnings_wanted() {
            into.push(warning(
                POSSIBLY_UNDEFINED_VARIABLE,
                at(&here, &field),
                possibly_undefined_variable_message(&read),
            ));
        }
    }
    // What the state machines nested in this state inherit: what reaching this state
    // had in scope, asked of its machine rather than copied into each of them.
    //
    // NOT this state's own `Assign`: only a `Map` and a `Parallel` declare a machine,
    // and their own assignments are evaluated when the work those machines DO has
    // completed, so a state inside one cannot count on them.
    let outer = Outer::State {
        machine: scopes,
        state: declared,
    };
    // A nested state machine's names reach the same history as this one's, so every
    // rule above holds for it too, and its states inherit this state's language — and
    // the variables this state has in scope, which the states it declares read.
    for member in ["ItemProcessor", "Iterator"] {
        if let Some(processor) = state.get(member) {
            diagnose_machine(
                processor,
                &nested_at(&here, format_args!("/{member}")),
                language,
                depth + 1,
                // A processor is the one position the language declares
                // `ProcessorConfig` at.
                true,
                // A processor the state runs in DISTRIBUTED mode is a scope BOUNDARY:
                // each of its items "starts a child workflow execution", which begins
                // with the variables such an execution begins with — none — so a state
                // inside it inherits nothing the enclosing scope assigned, whatever
                // that scope holds. The INLINE mode runs the processor inside this
                // execution, which is what inherits.
                match distributed_processor(processor) {
                    true => Outer::Nothing,
                    false => outer,
                },
                into,
            );
        }
    }
    if let Some(branches) = state.get("Branches").and_then(Value::as_array) {
        for (index, branch) in branches.iter().enumerate() {
            diagnose_machine(
                branch,
                &element_at(&here, "Branches", index),
                language,
                depth + 1,
                false,
                outer,
                into,
            );
        }
    }
}

/// Whether `processor` is one a `Map` state runs in DISTRIBUTED mode, which is the one
/// thing its `ProcessorConfig` says that the rules here read: an item of such a
/// processor "starts a child workflow execution", so what it inherits from the scope
/// around it is what a new execution inherits.
///
/// Read as the walk that RUNS a `Map` state reads it (`super`'s own `map_processor`),
/// mode names compared without regard to case, so the create-time analysis and the run
/// cannot disagree about which processors are a scope boundary.
fn distributed_processor(processor: &Value) -> bool {
    processor
        .get("ProcessorConfig")
        .and_then(|config| config.get("Mode"))
        .and_then(Value::as_str)
        .is_some_and(|mode| mode.eq_ignore_ascii_case(DISTRIBUTED_MODE))
}

/// The `ProcessorConfig` mode that runs each item of a `Map` state's processor as a
/// child workflow execution of its own.
const DISTRIBUTED_MODE: &str = "DISTRIBUTED";

/// The RULE rules of one `Choice` state: every entry of the array that holds its rules
/// is an object, and every field of such an entry is one the language declares on a
/// rule ([`CHOICE_RULE_FIELDS`]).
///
/// The rules nested under `And`, `Or` and `Not` are judged the same way and at their own
/// positions, `depth` bounding the nesting this follows exactly as every other
/// caller-shaped tree here is bounded. A field the language does not declare is reported
/// as the unsupported field it is, with the field it is a near miss of suggested
/// ([`unsupported_field_message`]) — a misspelled comparator is a rule that can never
/// decide anything, so it is refused where it is submitted rather than failing every
/// execution that reaches the state.
///
/// `transition` says whether a rule at this level owes the member that says where the
/// execution goes: a rule of the state's OWN `Choices` array is the whole of a branch of
/// the state's decision ("Each Choice Rule […] MUST have a "Next" field"), so one that
/// names no transition leaves the state with nowhere to go when it holds; a rule nested
/// under `And`, `Or` or `Not` is a CONDITION of the rule that holds it and decides no
/// transition of its own.
fn diagnose_choice_rules(
    rules: Option<&Value>,
    here: &str,
    member: &str,
    transition: bool,
    depth: usize,
    into: &mut Diagnosis,
) {
    let Some(depth) = depth.checked_sub(1) else {
        return;
    };
    let Some(rules) = rules else {
        return;
    };
    // A member holding a LIST of rules that is not one, and the one member that holds a
    // single rule (`Not`), are read as what they are: `Choices` being an array at all is
    // the state's own schema rule, reported where that array is read.
    let entries: Vec<&Value> = match rules.as_array() {
        Some(entries) => entries.iter().collect(),
        None if member == "Not" => vec![rules],
        None => return,
    };
    for (index, entry) in entries.into_iter().enumerate() {
        let at_entry = match rules.is_array() {
            true => element_at(here, member, index),
            false => nested_at(here, format_args!("/{member}")),
        };
        let Some(entry) = entry.as_object() else {
            into.push(error(
                SCHEMA_VALIDATION_FAILED,
                at(&at_entry, ""),
                entry_object_message(member),
            ));
            continue;
        };
        // The transition the rule owes is reported exactly as every other field an
        // object of the language owes is — the field, located at the object that owes it
        // ([`required_fields_message`]) — and then as the transition that is MISSING:
        // the rule points the execution nowhere, which is the defect of the transition
        // graph it is, reported under the code that graph's problems are reported under
        // and located at the member that would have named the target.
        if transition && !entry.contains_key(CHOICE_TRANSITION_MEMBER) {
            into.push(error(
                SCHEMA_VALIDATION_FAILED,
                at(&at_entry, ""),
                required_fields_message(&[CHOICE_TRANSITION_MEMBER]),
            ));
            into.push(error(
                MISSING_TRANSITION_TARGET,
                at(&at_entry, &format!("/{CHOICE_TRANSITION_MEMBER}")),
                MISSING_TRANSITION_POINTER_MESSAGE,
            ));
        }
        for (field, value) in into.order().members(&at_entry, entry) {
            if !CHOICE_RULE_FIELDS.contains(&field.as_str()) {
                into.push(error(
                    SCHEMA_VALIDATION_FAILED,
                    at(&at_entry, ""),
                    unsupported_field_message(field, CHOICE_RULE_FIELDS),
                ));
                continue;
            }
            if NESTED_RULE_MEMBERS.contains(&field.as_str()) {
                diagnose_choice_rules(Some(value), &at_entry, field, false, depth, into);
            }
        }
    }
}

/// The OBJECT rules of one state: every object it carries whose fields the language
/// declares ([`DECLARED_OBJECT_FIELDS`]) carries only those fields, and so does every such
/// object nested inside one of them.
///
/// `declared` is the state's own closed field set, so an object written at a member the
/// state's type does not declare at all is not judged here: that it is there is already
/// reported, and what it holds says nothing more about a state that cannot carry it.
/// The objects are judged in the order the definition writes them — the queue is drained
/// from the FRONT, so what is reported about one object follows what is reported about
/// the object written before it — and each diagnostic is located at the object that
/// carries the field, which is the position a client has to look at.
fn diagnose_declared_objects(
    state: &Map<String, Value>,
    declared: &[&str],
    here: &str,
    into: &mut Diagnosis,
) {
    let mut pending: ::std::collections::VecDeque<(String, &'static [&'static str], &Value)> =
        ::std::collections::VecDeque::new();
    for member in into.order().names(here, state) {
        if !declared.contains(&member.as_str()) {
            continue;
        }
        if let Some((fields, value)) = declared_object_fields(member).zip(state.get(member)) {
            pending.push_back((
                nested_at(here, format_args!("/{}", echoed(member))),
                fields,
                value,
            ));
        }
    }
    while let Some((at_object, fields, value)) = pending.pop_front() {
        let Some(members) = value.as_object() else {
            continue;
        };
        for field in into.order().names(&at_object, members) {
            if !fields.contains(&field.as_str()) {
                into.push(error(
                    SCHEMA_VALIDATION_FAILED,
                    at(&at_object, ""),
                    unsupported_field_message(field, fields),
                ));
                continue;
            }
            // An object nested inside this one is judged by its own set, exactly as
            // this one is judged by the state's, and after the objects beside it.
            if let Some((nested, value)) = declared_object_fields(field).zip(members.get(field)) {
                pending.push_back((
                    nested_at(&at_object, format_args!("/{}", echoed(field))),
                    nested,
                    value,
                ));
            }
        }
        // And the values this object's own fields carry, where the language names the
        // values one of them admits ([`NAMED_VALUE_FIELDS`]).
        diagnose_named_values(members, fields, &at_object, into);
    }
}

/// The INTEGRATION-PATTERN rule of one `Task` state: the pattern its `Resource` asks
/// for must be one the workflow type this definition is being created as can perform.
///
/// An integration pattern is what FINISHES a task: the run-a-job pattern (`.sync`)
/// waits for a job the call started to reach an end state, and the callback pattern
/// (`.waitForTaskToken`) waits for an answer from outside the execution. Both are
/// served for the STANDARD workflow type alone — an EXPRESS execution is a single
/// at-most-once run with no durable state to suspend — and both are stated by the one
/// sentence this service words an unsupported pattern with
/// ([`super::EXPRESS_CALLBACK_UNSUPPORTED`], [`super::EXPRESS_RUN_A_JOB_UNSUPPORTED`]),
/// so the create-time refusal and the data plane's own cannot word one rule two ways.
///
/// WHERE each is refused is decided by the INTEGRATION the `Resource` names. RECORDED:
/// an EXPRESS create naming the run-a-job pattern of an Amazon Web Services service's
/// integration (`arn:aws:states:::ecs:runTask.sync`) is refused HERE, where the document
/// is submitted, and so is one naming the callback pattern; the one run-a-job integration
/// a create is ACCEPTED on is this service's OWN nested execution
/// (`arn:aws:states:::states:startExecution.sync`,
/// [`crate::state::integration::names_nested_execution`]), whose state machine is created
/// and whose every execution then fails AT that state with the same sentence
/// ([`super::EXPRESS_RUN_A_JOB_UNSUPPORTED`], raised by the interpreter, which starts no
/// child) — so a client that deploys such a definition gets the state machine and sees
/// the failure per execution.
///
/// Reported under the code every defect of the language's SCHEMA is reported under
/// ([`SCHEMA_VALIDATION_FAILED`]), located at the member that asked for the pattern.
fn diagnose_task_resource(
    state: &Map<String, Value>,
    state_type: &str,
    here: &str,
    into: &mut Diagnosis,
) {
    if state_type != "Task" || !into.express() {
        return;
    }
    let Some(resource) = state.get(RESOURCE_MEMBER).and_then(Value::as_str) else {
        return;
    };
    let unsupported = match crate::state::integration::named_pattern(resource) {
        Some(crate::state::integration::CALLBACK_PATTERN) => super::EXPRESS_CALLBACK_UNSUPPORTED,
        Some(
            crate::state::integration::RUN_A_JOB_PATTERN
            | crate::state::integration::RUN_A_JOB_PATTERN_2,
        ) if !crate::state::integration::names_nested_execution(resource) => {
            super::EXPRESS_RUN_A_JOB_UNSUPPORTED
        }
        _ => return,
    };
    into.push(error(
        SCHEMA_VALIDATION_FAILED,
        at(here, &format!("/{RESOURCE_MEMBER}")),
        unsupported,
    ));
}

/// The ALTERNATIVE-field rule of one state: of a group of fields the language declares
/// as alternatives ([`EXCLUSIVE_FIELD_GROUPS`]), a state carries at most one.
///
/// The diagnostic is located at the STATE rather than at either field: the state is
/// what carries one field too many, and neither of the two is the one to remove.
///
/// A field is counted as DECLARED where the state writes the member at all, whatever
/// it wrote in it: the language gives each of these fields one admitted form — `"End"`
/// is only ever `true` — so a state that writes the member has declared the thing the
/// group is about there.
fn diagnose_exclusive_fields(
    state: &Map<String, Value>,
    state_type: &str,
    here: &str,
    into: &mut Diagnosis,
) {
    let declared = state_fields(state_type);
    for group in EXCLUSIVE_FIELD_GROUPS {
        // A group whose fields the type does not declare says nothing about a state of
        // that type: such a field earns the unsupported-field diagnostic instead.
        if !group.iter().all(|field| declared.contains(field)) {
            continue;
        }
        let carried = group
            .iter()
            .filter(|field| state.contains_key(**field))
            .count();
        if carried > 1 {
            into.push(error(
                SCHEMA_VALIDATION_FAILED,
                at(here, ""),
                one_of_fields_message(group),
            ));
        }
    }
}

/// The `Retry` and `Catch` rules of one state: what the language requires of the
/// retriers and catchers a state of this type declares — the fields each entry owes
/// and may carry, the errors it takes over, and the bounds the language documents for
/// a retrier's own figures.
///
/// A member the type does not declare at all is not judged here: that it is there is
/// already reported ([`state_fields`]), and what it holds says nothing more about a
/// definition whose state cannot carry it.
fn diagnose_error_handlers(
    state: &Map<String, Value>,
    state_type: &str,
    here: &str,
    into: &mut Diagnosis,
) {
    for member in [RETRY_MEMBER, CATCH_MEMBER] {
        if !state_fields(state_type).contains(&member) {
            continue;
        }
        let (fields, required) = if member == CATCH_MEMBER {
            (CATCHER_FIELDS, CATCHER_REQUIRED_FIELDS)
        } else {
            (RETRIER_FIELDS, RETRIER_REQUIRED_FIELDS)
        };
        let Some(declared) = state.get(member) else {
            continue;
        };
        // The member holds a LIST of entries, so a value that is not one carries no
        // entry to judge: it is reported as the member it is instead.
        let Some(entries) = declared.as_array() else {
            into.push(error(
                SCHEMA_VALIDATION_FAILED,
                at(here, &format!("/{member}")),
                array_member_message(member),
            ));
            continue;
        };
        for (index, entry) in entries.iter().enumerate() {
            let at_entry = element_at(here, member, index);
            let Some(entry) = entry.as_object() else {
                into.push(error(
                    SCHEMA_VALIDATION_FAILED,
                    at(&at_entry, ""),
                    entry_object_message(member),
                ));
                continue;
            };
            // An entry of either array is an object the language declares a closed set
            // of fields on, reported exactly as a state machine object's and a state's
            // are: the fields it owes together, each field outside the set on its own,
            // both located at the ENTRY the fields belong to.
            let missing: Vec<&str> = required
                .iter()
                .copied()
                .filter(|field| !entry.contains_key(*field))
                .collect();
            if !missing.is_empty() {
                into.push(error(
                    SCHEMA_VALIDATION_FAILED,
                    at(&at_entry, ""),
                    required_fields_message(&missing),
                ));
            }
            for field in into.order().names(&at_entry, entry) {
                if !fields.contains(&field.as_str()) {
                    into.push(error(
                        SCHEMA_VALIDATION_FAILED,
                        at(&at_entry, ""),
                        unsupported_field_message(field, fields),
                    ));
                }
            }
            // `ErrorEquals` names the errors the entry takes over, so what it holds is
            // a non-empty list of error NAMES: a list that is neither is reported at
            // the member, and one name that is not a string at that name's own
            // position. The checks below it are about OTHER members of the same entry,
            // so what this one has nothing left to say about leaves the block rather
            // than the entry.
            'error_equals: {
                let Some(declared) = entry.get(ERROR_EQUALS_MEMBER) else {
                    break 'error_equals;
                };
                let array = nested_at(&at_entry, format_args!("/{ERROR_EQUALS_MEMBER}"));
                let Some(names) = declared.as_array() else {
                    into.push(error(
                        SCHEMA_VALIDATION_FAILED,
                        at(&array, ""),
                        array_member_message(ERROR_EQUALS_MEMBER),
                    ));
                    break 'error_equals;
                };
                if names.is_empty() {
                    into.push(error(
                        SCHEMA_VALIDATION_FAILED,
                        at(&array, ""),
                        EMPTY_VALUE_MESSAGE,
                    ));
                }
                for (position, name) in names.iter().enumerate() {
                    if !name.is_string() {
                        into.push(error(
                            SCHEMA_VALIDATION_FAILED,
                            at(&indexed_at(&array, position), ""),
                            ERROR_NAME_MESSAGE,
                        ));
                    }
                }
                // `States.ALL` matches every error there is, so the language admits it
                // only as the ONE entry of the list it is named in, and only in the LAST
                // entry of the array that holds that list: a name beside it, and an entry
                // after it, can never decide anything. Each entry that names it outside
                // those two positions is reported at the name itself, which is what the
                // caller has to move.
                if names.len() < 2 && index + 1 == entries.len() {
                    break 'error_equals;
                }
                for (position, name) in names.iter().enumerate() {
                    if name.as_str() == Some(ALL_ERRORS) {
                        into.push(error(
                            SCHEMA_VALIDATION_FAILED,
                            at(&indexed_at(&array, position), ""),
                            ALL_ERRORS_ALONE_MESSAGE,
                        ));
                    }
                }
            }
            // A retrier's own figures carry NUMBERS the language bounds, so one the
            // definition wrote outside its bound is a schema error of this entry,
            // reported at the figure itself ([`RETRIER_FIGURES`], judged by the one
            // checker every bounded figure of a definition is judged by). Each is
            // judged only where the entry DECLARES it: a catcher carries none of them,
            // and one written there is reported as the unsupported field it is above
            // rather than twice.
            diagnose_figures(entry, RETRIER_FIGURES, fields, &at_entry, into);
            // And its `JitterStrategy` names one of the two strategies the language
            // has, judged by the one checker every named-value field is judged by
            // ([`NAMED_VALUE_FIELDS`]) and only where the entry declares it — a catcher
            // declares none of these fields.
            diagnose_named_values(entry, fields, &at_entry, into);
        }
    }
}

/// The most distinct variables one state machine's scope analysis follows through its
/// state graph ([`Scopes`]).
///
/// Following one variable costs one walk of the graph and a definition is
/// caller-sized, so the number followed is bounded like every other dimension of this
/// walk, and the bound is the one the reported list already carries
/// ([`DEFAULT_MAX_DIAGNOSTICS`]). A variable NO state assigns needs no walk at all and
/// is not counted against it: it is out of scope wherever it is read. A variable past
/// the bound is taken to be in scope, so the bound can only leave a warning unsaid,
/// never invent one.
///
/// It is also what fits one state's scope in a single [`ScopeMask`], which is what
/// keeps the analysis from holding the PRODUCT of the state count and the variable
/// count.
const MAX_SCOPED_VARIABLES: usize = DEFAULT_MAX_DIAGNOSTICS;

/// The most states the variable-scope analysis visits across ONE diagnosis.
///
/// Following one variable costs one walk of its machine's graph, so the analysis's
/// cost is the PRODUCT of the variables it follows and the states it walks, and a
/// definition chooses both — as it chooses how many machines it nests. The product is
/// therefore spent from ONE budget for the whole diagnosis
/// ([`Diagnosis::spend_scope_visits`]) rather than bounded per machine: the budget is
/// the diagnostics one call reports ([`DEFAULT_MAX_DIAGNOSTICS`]) times
/// [`MAX_ANALYSED_STATES`], which is the largest state graph this analysis walks at
/// all. Past it no further variable is followed, and a variable that was not followed
/// is taken to be in scope, so a spent budget can only leave a warning unsaid.
const MAX_SCOPE_VISITS: usize = DEFAULT_MAX_DIAGNOSTICS * MAX_ANALYSED_STATES;

/// The most states of one definition this create-time analysis walks.
///
/// Emulator bound on the ANALYSIS, not on a run: a definition is bounded only at the
/// modelled 1 MiB, which is room for more states than following every variable through
/// all of them is worth, so the walk stops here and says nothing further.
const MAX_ANALYSED_STATES: usize = 1_000;

/// Which of the variables one machine's analysis followed a state can count on: one
/// BIT each, at the index [`Scopes::assigned`] records for the variable.
///
/// A set of NAMES per state would cost the product of the state count and the variable
/// count, and a definition chooses both, so a state's scope is a mask: the analysis
/// holds one of these per state and nothing that grows with the names.
type ScopeMask = u128;

/// [`MAX_SCOPED_VARIABLES`] variables must fit the mask that carries them.
const _: () = assert!(MAX_SCOPED_VARIABLES <= ScopeMask::BITS as usize);

/// The member a state assigns variables in: "Assign".
pub(super) const ASSIGN_MEMBER: &str = "Assign";

/// The member a JSONata state's own output is built in: "Output".
pub(super) const OUTPUT_MEMBER: &str = "Output";

/// The member a JSONata `Task`, `Map` or `Parallel` state builds the document it hands
/// its work in: "Arguments".
pub(super) const ARGUMENTS_MEMBER: &str = "Arguments";

/// The member a `Map` state projects each of its items through: "ItemSelector".
pub(super) const ITEM_SELECTOR_MEMBER: &str = "ItemSelector";

/// The member a `Task` state names the role its work is made under in: "Credentials",
/// and the one field of it that names that role.
pub(super) const CREDENTIALS_MEMBER: &str = "Credentials";
/// See [`CREDENTIALS_MEMBER`].
pub(super) const ROLE_ARN_FIELD: &str = "RoleArn";

/// What was already in scope where a state machine is DECLARED, which its states
/// inherit.
///
/// A QUESTION asked up the chain of enclosing states, never a set copied into each
/// level: a copy per level would hold the caller's variable names once per nesting
/// level, and the nesting is the caller's too.
#[derive(Clone, Copy)]
pub(super) enum Outer<'a> {
    /// The definition itself: a run has assigned nothing where it starts.
    Nothing,
    /// A machine one state declares: what reaching that state had in scope, which is
    /// its own machine's answer for it.
    State {
        machine: &'a Scopes<'a>,
        state: &'a str,
    },
}

impl Outer<'_> {
    /// Whether the enclosing scope holds `name`.
    fn holds(&self, name: &str) -> bool {
        match self {
            Outer::Nothing => false,
            Outer::State { machine, state } => machine.holds(state, name),
        }
    }

    /// Whether SOME run reaching the enclosing state had assigned `name`.
    fn may_hold(&self, name: &str) -> bool {
        match self {
            Outer::Nothing => false,
            Outer::State { machine, state } => machine.may_hold(state, name),
        }
    }
}

/// Which variables each state of ONE state machine can count on having been assigned
/// by the time it runs: those EVERY path from the machine's entry point to that state
/// assigns, together with the ones inherited from the state that declares the machine.
///
/// A variable is in a state's scope only when no run can reach that state without
/// having assigned it — which is what makes the ones outside it "possibly" undefined:
/// the read resolves on some runs and not on others. A state no run reaches has
/// nothing but the inherited variables in scope.
///
/// It answers the WEAKER question too ([`Scopes::may_hold`]): whether SOME run reaching
/// the state has assigned the variable, which is what tells a read that resolves on some
/// runs from one that could resolve on none.
pub(super) struct Scopes<'a> {
    /// Every variable some state of this machine assigns, and which bit of a state's
    /// [`ScopeMask`] answers for it. A variable the analysis did NOT follow — the
    /// enclosing scope already holds it, or the bound or the budget is spent — carries
    /// no bit and is taken to be in scope.
    assigned: BTreeMap<String, Option<usize>>,
    /// Per state, which of the followed variables every run reaching it has assigned.
    masks: BTreeMap<&'a str, ScopeMask>,
    /// Per state, which of them SOME run reaching it has assigned: the weaker question,
    /// and the one that tells a name no run could have assigned from one a run could.
    possible: BTreeMap<&'a str, ScopeMask>,
    /// What this machine is declared inside.
    outer: Outer<'a>,
}

impl<'a> Scopes<'a> {
    /// Follow each variable `states` assigns through the transition graph from
    /// `start_at`, with `outer` already in scope.
    ///
    /// Each variable is followed with ONE walk of the graph, from the entry point
    /// along the transitions that do NOT assign it, and each walk is spent from the
    /// diagnosis's own [`MAX_SCOPE_VISITS`] budget, so the analysis holds one walk's
    /// states at a time and costs the same whatever the definition declares.
    fn of(
        states: &'a Map<String, Value>,
        start_at: Option<&str>,
        outer: Outer<'a>,
        into: &mut Diagnosis,
    ) -> Self {
        // Which assignment block runs on the way along each transition: the state's own
        // when its work completed, a `Catch` catcher's when it did not, and a `Choice`
        // rule's for the rule that matched. The blocks are held ONCE and the transitions
        // name them by index, so a state with many transitions does not hold its block
        // once per transition.
        let mut blocks: Vec<BTreeSet<String>> = Vec::new();
        let mut graph: BTreeMap<&str, Vec<ScopeEdge>> = BTreeMap::new();
        for (name, state) in states {
            let mut out = Vec::new();
            if let Some(state) = state.as_object() {
                let Assignments {
                    own,
                    caught,
                    chosen,
                } = assignments(state);
                let own_block = blocks.len();
                blocks.push(own);
                let mut held = |of: Vec<BTreeSet<String>>| -> Vec<usize> {
                    of.into_iter()
                        .map(|block| {
                            blocks.push(block);
                            blocks.len() - 1
                        })
                        .collect()
                };
                let caught_blocks = held(caught);
                let chosen_blocks = held(chosen);
                for transition in transitions(state) {
                    if !states.contains_key(transition.target.as_str()) {
                        continue;
                    }
                    let assigns = match transition.assigns {
                        AssignedOnTheWay::Own => Some(own_block),
                        AssignedOnTheWay::Catcher(index) => caught_blocks.get(index).copied(),
                        AssignedOnTheWay::ChoiceRule(index) => chosen_blocks.get(index).copied(),
                    };
                    out.push(ScopeEdge {
                        target: transition.target,
                        assigns: assigns.unwrap_or(own_block),
                    });
                }
            }
            graph.insert(name.as_str(), out);
        }
        // Each walk below visits each state at most once, so one walk costs at most
        // this many visits of the budget; following one variable takes TWO of them — one
        // for the states that reach it unassigned and one for the states some
        // assignment of it reaches.
        let per_walk = states.len();
        let per_variable = per_walk.saturating_mul(2);
        let mut scopes = Self {
            assigned: BTreeMap::new(),
            masks: states.keys().map(|name| (name.as_str(), 0)).collect(),
            possible: states.keys().map(|name| (name.as_str(), 0)).collect(),
            outer,
        };
        // Every variable this machine assigns somewhere. One set per MACHINE, so what
        // this holds is the machine's own text rather than that text once per state.
        let declared: BTreeSet<&String> = blocks.iter().flatten().collect();
        if !into.spend_scope_visits(per_walk) {
            // Nothing can be followed, so nothing this machine assigns is judged.
            scopes.assigned = declared.into_iter().map(|name| (name.clone(), None)).collect();
            return scopes;
        }
        // A machine whose entry point names no declared state has no run to reason
        // about, so no state of it reaches a read with anything assigned.
        let entry = start_at.filter(|start_at| states.contains_key(*start_at));
        let entries: Vec<&str> = entry.into_iter().collect();
        // Where a run can be at all: the states a read of a variable is judged in.
        let reachable = reached_from(&entries, &graph, &blocks, None);
        let mut followed = 0_usize;
        for name in declared {
            // A variable the enclosing scope already holds is in scope at every state
            // of this machine whatever this machine does with it, and one the bound or
            // the budget cannot reach is taken to be, so neither is followed.
            if followed >= MAX_SCOPED_VARIABLES
                || scopes.outer.holds(name)
                || !into.spend_scope_visits(per_variable)
            {
                scopes.assigned.insert(name.clone(), None);
                continue;
            }
            let index = followed;
            followed += 1;
            // Where the variable is NOT yet assigned: every state a run can reach
            // without having taken a transition that assigns it.
            let unassigned = reached_from(&entries, &graph, &blocks, Some(name));
            // And where SOME run has assigned it: every state reachable from the far
            // side of a transition that assigns it, which is the states a run could
            // arrive at carrying the variable.
            let assigned_at: Vec<&str> = graph
                .iter()
                .filter(|(state, _)| reachable.contains(**state))
                .flat_map(|(_, edges)| edges.iter())
                .filter(|edge| {
                    blocks
                        .get(edge.assigns)
                        .is_some_and(|block| block.contains(name))
                })
                .map(|edge| edge.target.as_str())
                .collect();
            let assigned_before = reached_from(&assigned_at, &graph, &blocks, None);
            for (state, mask) in &mut scopes.masks {
                if reachable.contains(state) && !unassigned.contains(state) {
                    *mask |= 1 << index;
                }
            }
            for (state, mask) in &mut scopes.possible {
                if reachable.contains(state) && assigned_before.contains(state) {
                    *mask |= 1 << index;
                }
            }
            scopes.assigned.insert(name.clone(), Some(index));
        }
        scopes
    }

    /// Whether `state` can count on `name` having been assigned by the time it runs.
    fn holds(&self, state: &str, name: &str) -> bool {
        match self.assigned.get(name) {
            // A variable this machine assigns and the analysis followed: in scope
            // where every run reaching the state has passed an assignment of it.
            Some(Some(index)) => {
                self.masks.get(state).copied().unwrap_or(0) & (1 << index) != 0
            }
            // One it did not follow, which is taken to be in scope.
            Some(None) => true,
            // One no state of this machine assigns: only the enclosing scope can hold
            // it.
            None => self.outer.holds(name),
        }
    }

    /// Whether SOME run reaching `state` has assigned `name` by the time it runs.
    ///
    /// The weaker question [`Scopes::holds`] asks, and the one that tells a read the
    /// definition leaves with NOTHING to resolve from a read that resolves on some runs
    /// of it and not on others: a variable this machine assigns somewhere upstream of
    /// `state` is one a run can arrive carrying, however many other runs arrive without
    /// it.
    fn may_hold(&self, state: &str, name: &str) -> bool {
        match self.assigned.get(name) {
            Some(Some(index)) => {
                self.possible.get(state).copied().unwrap_or(0) & (1 << index) != 0
            }
            Some(None) => true,
            None => self.outer.may_hold(name),
        }
    }
}

/// One transition of a machine's graph as the scope analysis walks it: where it goes,
/// and which of the machine's assignment blocks runs on the way there.
struct ScopeEdge {
    target: String,
    /// Index into the machine's blocks.
    assigns: usize,
}

/// The states one run can reach from any of `entries`, following `graph` — past a
/// transition whose own block assigns `without`, that run has assigned it, so such a
/// transition is not followed.
///
/// `without` `None` makes this the states reachable at all. The walk visits each state
/// once, so it costs one pass over the graph whatever it was entered from.
fn reached_from<'a>(
    entries: &[&str],
    graph: &BTreeMap<&'a str, Vec<ScopeEdge>>,
    blocks: &[BTreeSet<String>],
    without: Option<&str>,
) -> BTreeSet<&'a str> {
    let mut reached = BTreeSet::new();
    let mut pending: Vec<&str> = entries.to_vec();
    while let Some(state) = pending.pop() {
        let Some((state, edges)) = graph.get_key_value(state) else {
            continue;
        };
        if !reached.insert(*state) {
            continue;
        }
        for edge in edges {
            let assigned = without.is_some_and(|name| {
                blocks
                    .get(edge.assigns)
                    .is_some_and(|block| block.contains(name))
            });
            if !assigned {
                pending.push(edge.target.as_str());
            }
        }
    }
    reached
}

/// What one state ASSIGNS, by the path the assignment runs on.
///
/// A key written as a JSONPath reference field (`"<name>.$"`) assigns the variable
/// `<name>`, the suffix being how that language spells a value read from a path. A
/// state machine NESTED in this state assigns in a scope of its own, so its states'
/// blocks are not read here.
struct Assignments {
    /// The keys of the state's own `Assign` block, which the language evaluates when
    /// the state's work completed.
    own: BTreeSet<String>,
    /// The keys of each `Catch` catcher's own `Assign`, in the catchers' own order,
    /// which are evaluated INSTEAD when the work failed and that catcher took over.
    caught: Vec<BTreeSet<String>>,
    /// The keys of each `Choice` rule's own `Assign`, in the rules' own order, which are
    /// evaluated for the ONE rule that matched — "you can use Assign in a Choice rule" —
    /// so they are the assignments made on the way along that rule's transition.
    chosen: Vec<BTreeSet<String>>,
}

/// The ASSIGNMENT rules of one state: every block of assignments it declares is a JSON
/// OBJECT, and every name it assigns is a name the language admits for a variable.
///
/// "Assign […] store values in variables" is a block of name-to-value members, so a
/// value of any other shape declares no assignment at all, and a KEY the language
/// cannot read as a variable name declares a variable nothing could ever read: both are
/// defects of the submitted definition, reported at the block that carries them. A
/// JSONPath block spells a member read from a path as `"<name>.$"`, so the suffix is
/// dropped before the name is judged, exactly as the scope analysis reads it
/// ([`assignments`]).
///
/// The blocks are the state's OWN and each `Catch` catcher's, which are the two
/// positions the language declares assignments at; the member is judged only for a
/// state whose TYPE declares it, since a block written where the type carries no
/// `Assign` is reported as the unsupported field it is.
fn diagnose_assignments(
    state: &Map<String, Value>,
    state_type: &str,
    here: &str,
    into: &mut Diagnosis,
) {
    if !state_fields(state_type).contains(&ASSIGN_MEMBER) {
        return;
    }
    let catchers = state
        .get(CATCH_MEMBER)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut blocks: Vec<(String, &Value)> = Vec::new();
    if let Some(block) = state.get(ASSIGN_MEMBER) {
        blocks.push((nested_at(here, format_args!("/{ASSIGN_MEMBER}")), block));
    }
    for (index, catcher) in catchers.iter().enumerate() {
        if let Some(block) = catcher.get(ASSIGN_MEMBER) {
            blocks.push((
                nested_at(
                    &element_at(here, CATCH_MEMBER, index),
                    format_args!("/{ASSIGN_MEMBER}"),
                ),
                block,
            ));
        }
    }
    for (at_block, block) in blocks {
        let Some(assigned) = block.as_object() else {
            into.push(error(
                SCHEMA_VALIDATION_FAILED,
                at(&at_block, ""),
                format!("'{ASSIGN_MEMBER}' must be a JSON object of variables"),
            ));
            continue;
        };
        for name in into.order().names(&at_block, assigned) {
            let declared = name.strip_suffix(".$").unwrap_or(name);
            if !super::is_variable_name(declared) {
                into.push(error(
                    SCHEMA_VALIDATION_FAILED,
                    at(&at_block, ""),
                    invalid_variable_name_message(declared),
                ));
            }
        }
    }
}

/// [`Assignments`] of one state.
fn assignments(state: &Map<String, Value>) -> Assignments {
    let names = |block: Option<&Value>| -> BTreeSet<String> {
        block
            .and_then(Value::as_object)
            .map(|block| {
                block
                    .keys()
                    .map(|key| key.strip_suffix(".$").unwrap_or(key).to_string())
                    .collect()
            })
            .unwrap_or_default()
    };
    let blocks_of = |member: &str| -> Vec<BTreeSet<String>> {
        state
            .get(member)
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .map(|entry| names(entry.get(ASSIGN_MEMBER)))
            .collect()
    };
    Assignments {
        own: names(state.get(ASSIGN_MEMBER)),
        caught: blocks_of(CATCH_MEMBER),
        chosen: blocks_of(CHOICES_MEMBER),
    }
}

/// Every variable one state can assign, on whichever path it runs: what its OWN
/// fields may read, since the language evaluates a state's assignments — its own, or
/// its catcher's where that catcher took over — before the output they feed.
///
/// Which of them a state the transition LEADS TO can count on is a different question,
/// and the one [`Scopes`] answers per transition.
fn assigned_variables(state: &Map<String, Value>) -> BTreeSet<String> {
    let Assignments {
        mut own,
        caught,
        chosen: _,
    } = assignments(state);
    for block in caught {
        own.extend(block);
    }
    own
}

/// Every variable one state's own fields READ: the path from the state to the field
/// that reads it, and the variable's name, each pair once and in the order the state
/// — at `state_at`, in the order `order` says the definition writes it — writes those
/// fields.
///
/// A read is what [`super::variable_references`] reports a state referencing a
/// variable through, so the lint below and `DescribeStateMachine`'s
/// `variableReferences` cannot disagree about what a read is: a `$name` term of a
/// JSONata expression (which the language delimits with `{%` and `%}`) or of a
/// JSONPath reference field (a member whose name ends in `.$`). A state machine NESTED
/// in this state is walked as the states it declares, so its reads are reported under
/// their own names.
///
/// The walk is the one [`walk_fields`] does for every field this module reads, so the
/// path a field is reported at is the same one every other diagnostic about it
/// carries.
///
/// A `Choice` state's own rules are entered member by member rather than as one value
/// ([`choice_rule_roots`]), because WHICH of a rule's members resolves a path is the
/// rule's own fact: a generic descent reads only the reference-field suffix, which a
/// rule's `Variable` and its `*Path` operands do not carry.
fn variable_reads(
    state: &Map<String, Value>,
    state_at: &str,
    order: &MemberOrder,
) -> Vec<(String, (String, bool))> {
    let mut found: BTreeSet<(String, String, bool)> = BTreeSet::new();
    let mut roots: Vec<(String, &Value, bool)> = Vec::new();
    for (member, value) in state
        .iter()
        .filter(|(member, _)| !NESTED_MACHINE_MEMBERS.contains(&member.as_str()))
    {
        let at_member = nested_at("", format_args!("/{}", echoed(member)));
        match (member.as_str(), value.as_array()) {
            (CHOICES_MEMBER, Some(rules)) => {
                for (index, rule) in rules.iter().enumerate() {
                    choice_rule_roots(
                        rule,
                        &indexed_at(&at_member, index),
                        MAX_EXPRESSION_DEPTH,
                        &mut roots,
                    );
                }
            }
            _ => roots.push((at_member, value, reads_a_path(member))),
        }
    }
    walk_fields(roots, |path, text, reference_field| {
        let mut names = BTreeSet::new();
        for expression in jsonata_expressions(text) {
            scan_expression(expression, &mut names);
        }
        if reference_field {
            scan_expression(text, &mut names);
        }
        for name in names {
            found.insert((path.to_string(), name, reference_field));
        }
    });
    let mut reads: Vec<(String, (String, bool))> = found
        .into_iter()
        .map(|(path, name, reference_field)| (path, (name, reference_field)))
        .collect();
    order.paths(state_at, &mut reads);
    reads
}

/// The roots one `Choice` rule contributes to [`variable_reads`]: each member of the
/// rule, at its own position, paired with whether the language RESOLVES a path at that
/// member ([`reads_a_path`]) — and the same for every rule nested in its `And`, `Or`
/// and `Not`.
///
/// A rule names the value its comparator tests through `Variable` and reads a second
/// operand through a `*Path` comparator, neither of which carries the reference-field
/// suffix a generic descent reads: entering the rule member by member is what makes a
/// variable named THERE a path the language resolves rather than a literal, exactly as
/// it is at a state's own `*Path` member.
///
/// `depth` bounds the nesting this follows, as every other walk of a caller-shaped rule
/// tree here is bounded.
fn choice_rule_roots<'a>(
    rule: &'a Value,
    at_rule: &str,
    depth: usize,
    into: &mut Vec<(String, &'a Value, bool)>,
) {
    let Some(depth) = depth.checked_sub(1) else {
        return;
    };
    let Some(members) = rule.as_object() else {
        return;
    };
    for (member, value) in members {
        let at_member = nested_at(at_rule, format_args!("/{}", echoed(member)));
        match (member.as_str(), value.as_array()) {
            (NESTED_RULE_MEMBER_AND | NESTED_RULE_MEMBER_OR, Some(nested)) => {
                for (index, rule) in nested.iter().enumerate() {
                    choice_rule_roots(rule, &indexed_at(&at_member, index), depth, into);
                }
            }
            (NESTED_RULE_MEMBER_NOT, _) => choice_rule_roots(value, &at_member, depth, into),
            _ => into.push((at_member, value, reads_a_path(member))),
        }
    }
}

/// The member a `Choice` state holds its rules in.
const CHOICES_MEMBER: &str = "Choices";

/// The three members of a `Choice` rule that carry rules of their own
/// ([`NESTED_RULE_MEMBERS`] is the same three as the set a rule is judged against).
const NESTED_RULE_MEMBER_AND: &str = "And";
/// See [`NESTED_RULE_MEMBER_AND`].
const NESTED_RULE_MEMBER_OR: &str = "Or";
/// See [`NESTED_RULE_MEMBER_AND`].
const NESTED_RULE_MEMBER_NOT: &str = "Not";

/// Whether the value of the state member `member` is a PATH the language resolves,
/// rather than a literal or an expression it evaluates.
///
/// The two ways the JSONPath half of the Amazon States Language writes a path at a
/// member of a state: the `*Path` family, whose value IS the path the member reads its
/// figure or its document from, and the reference field (`"<name>.$"`), which is how a
/// payload template says that a member it renders is read from one. A `Choice` rule's
/// `Variable` names the value its comparator tests the same way.
///
/// What this decides is whether a `$name` term written there is a variable the language
/// RESOLVES — so a variable nothing in scope assigns is an error rather than an
/// expression that evaluates to nothing ([`diagnose_state`]).
fn reads_a_path(member: &str) -> bool {
    member.ends_with(".$") || member.ends_with("Path") || member == CHOICE_VARIABLE_MEMBER
}

/// The member a JSONPath `Choice` rule names the value it tests with.
const CHOICE_VARIABLE_MEMBER: &str = "Variable";

/// The query language `declared` names, `None` when the member is absent, or why it
/// is not a query language the Amazon States Language defines.
pub(super) fn declared_language(declared: Option<&Value>) -> Result<Option<&'static str>, String> {
    let Some(declared) = declared else {
        return Ok(None);
    };
    let Some(named) = declared.as_str() else {
        return Err("'QueryLanguage' must be a string".to_string());
    };
    QUERY_LANGUAGES
        .iter()
        .find(|language| **language == named)
        .copied()
        .map(Some)
        .ok_or_else(|| {
            format!(
                "'QueryLanguage' must be one of {}, and this definition names '{}'",
                QUERY_LANGUAGES.join(", "),
                echoed(named)
            )
        })
}

/// The members a state reads its data through in JSONPath, which a state written in
/// JSONata must not carry: that language reads its data through members of its own
/// (`Arguments`, `Output`, `Items`, a `Choice` rule's `Condition`), so each of these
/// is a field of the OTHER query language.
///
/// These are the JSONPath data-flow members the Amazon States Language declares, as
/// [`super::executable_members`] lists them per state type: the `*Path` family, the
/// payload templates and `Pass`'s static `Result`.
pub(super) const JSONPATH_ONLY_MEMBERS: &[&str] = &[
    "CausePath",
    "ErrorPath",
    "HeartbeatSecondsPath",
    "InputPath",
    "ItemsPath",
    "MaxConcurrencyPath",
    "OutputPath",
    "Parameters",
    "Result",
    "ResultPath",
    "ResultSelector",
    "SecondsPath",
    "TimeoutSecondsPath",
    "TimestampPath",
    "ToleratedFailureCountPath",
    "ToleratedFailurePercentagePath",
];

/// The members that declare a state machine of their own, walked as one rather than
/// scanned as this state's data.
///
/// `pub(in crate::state)`, not private to this module: every reader that has to descend
/// a definition's nested state lists — the diagnostics here, the one that echoes a
/// document the definition wrote, the one that finds the state an event belongs to —
/// descends the SAME members, so there is one statement of which they are. A member
/// naming ONE machine carries it as an object and `Branches` carries a list of them, so
/// a reader takes both shapes.
pub(in crate::state) const NESTED_MACHINE_MEMBERS: &[&str] =
    &["Branches", "ItemProcessor", "Iterator"];

/// The member a state hands a failure to, whose own `Assign` and `Output` are the
/// only ones the error output is in scope in.
pub(super) const CATCH_MEMBER: &str = "Catch";

/// The member a state declares the retries of its own work in.
pub(super) const RETRY_MEMBER: &str = "Retry";

/// The member a retrier and a catcher name the errors they take over in.
pub(super) const ERROR_EQUALS_MEMBER: &str = "ErrorEquals";

/// The reserved term a JSONata expression reads the error of a caught failure
/// through, which the language scopes to a `Catch`: `$states.errorOutput` holds "the
/// error output" of the state that failed, and only that `Catch`'s own `Assign` and
/// `Output` are evaluated with it in scope.
const ERROR_OUTPUT: &str = "$states.errorOutput";

/// The reserved term a JSONata expression reads the state's own outcome through,
/// which the language binds only where there IS one: `$states.result` holds what the
/// state's work produced, so it is in scope in a state whose work produces a result
/// ([`RESULT_STATE_TYPES`]) and on that state's success path — never in a `Catch`,
/// which runs because the work produced no result at all.
const RESULT: &str = "$states.result";

/// The state types whose work produces a result, which is what binds [`RESULT`]: a
/// `Task`'s own work, and the per-item and per-branch results a `Map` and a
/// `Parallel` gather. Every other type carries its input through, so there is no
/// result for an expression of it to read.
const RESULT_STATE_TYPES: &[&str] = &["Map", "Parallel", "Task"];

/// How the service states a field an expression reads that the expression's scope does
/// not bind: the field is not there to read, so it is reported as the field it named.
///
/// `field` is the whole field expression as the definition wrote it — the reserved term
/// together with the member accessors written after it ([`super::field_read`]) —
/// because that, and not the term it starts with, is what the caller has to change.
pub(super) fn field_does_not_exist(field: &str) -> String {
    format!("Field '{field}' does not exist.")
}

/// The reserved terms of the state's own outcome that a `member` of a state of this
/// type must NOT read.
///
/// One rule, read off the member and the state's type: a term is reported wherever
/// the language does not bind it, so an expression that could never resolve is
/// refused at validation time rather than at run time.
pub(super) fn out_of_scope_terms(state_type: &str, member: &str) -> Vec<&'static str> {
    let caught = member == CATCH_MEMBER;
    let mut terms = Vec::new();
    if !caught {
        terms.push(ERROR_OUTPUT);
    }
    if caught || !RESULT_STATE_TYPES.contains(&state_type) {
        terms.push(RESULT);
    }
    terms
}

/// The rules a state written in JSONata must satisfy: it carries none of the other
/// language's data-flow members, and each reserved term of its own outcome is read
/// only where the language binds it ([`out_of_scope_terms`]).
fn diagnose_jsonata_state(
    name: &str,
    state: &Map<String, Value>,
    state_type: &str,
    here: &str,
    into: &mut Diagnosis,
) {
    for member in into.order().names(here, state) {
        // A `"<name>.$"` member is a JSONPath reference field, which is how that
        // language — and only that one — spells a member read from the state's data.
        if JSONPATH_ONLY_MEMBERS.contains(&member.as_str()) || member.ends_with(".$") {
            let member = echoed(member);
            into.push(error(
                SCHEMA_VALIDATION_FAILED,
                at(here, &format!("/{member}")),
                format!(
                    "state '{name}' is written in '{JSONATA}', so it must not carry the \
                     '{JSONPATH}' member '{member}'"
                ),
            ));
        }
    }
    if state_type == "Choice" {
        if let Some(rules) = state.get("Choices").and_then(Value::as_array) {
            for (index, rule) in rules.iter().enumerate() {
                diagnose_jsonata_rule(
                    name,
                    rule,
                    &element_at(here, "Choices", index),
                    MAX_EXPRESSION_DEPTH,
                    into,
                );
            }
        }
    }
    for (member, value) in into.order().members(here, state) {
        if NESTED_MACHINE_MEMBERS.contains(&member.as_str()) {
            continue;
        }
        let shown = echoed(member);
        // A member the language declares ONE SHAPE for carries a value of that shape,
        // or one JSONata expression that evaluates to one: "Arguments […] used to pass
        // information to the API actions of connected resources" is a document, `Items`
        // the array a `Map` iterates, and a value of any other shape declares work the
        // language cannot do with it. A STRING counts as the declared shape only where
        // it IS an expression: a string that opens no expression is the literal it is
        // written as, which is a document and an array in neither case.
        if let Some(declared) = declared_member_shape(member) {
            if let Some(why) = declared.unsatisfied(member, value) {
                into.push(error(
                    SCHEMA_VALIDATION_FAILED,
                    at(here, &format!("/{shown}")),
                    why,
                ));
            }
        }
        // Every field of this member whose text OPENS a JSONata expression without
        // being one: the language embeds an expression as the WHOLE value of a field,
        // between `{%` and `%}`, so a value that carries those marks anywhere else is
        // neither the expression it reads as nor the literal it would otherwise be.
        // Reported before the parse below, which is about the expression INSIDE those
        // marks. A member whose value NAMES something — a state, a type, a language, a
        // comment — holds no expression at all, so its text is left as written.
        if !NAMING_MEMBERS.contains(&member.as_str()) {
            for (field, text) in malformed_expressions(
                value,
                &nested_at(here, format_args!("/{shown}")),
                into.order(),
            ) {
                into.push(error(
                    SCHEMA_VALIDATION_FAILED,
                    at(here, &format!("/{shown}{field}")),
                    malformed_expression_message(&text),
                ));
            }
        }
        // The expressions of this member are PARSED first: a field whose text the
        // language cannot parse at all could never be evaluated, whatever data a run
        // reaches it with, so it is a defect of the submitted definition rather than an
        // outcome of running it. One diagnostic per FIELD, which is the granularity
        // `location` documents, located at that field.
        let unparsed = unparsed_fields(value, &nested_at(here, format_args!("/{shown}")), into);
        for (field, why) in unparsed {
            into.push(error(
                INVALID_JSONATA_EXPRESSION,
                at(here, &format!("/{shown}{field}")),
                why,
            ));
        }
        for term in out_of_scope_terms(state_type, member) {
            // One diagnostic per FIELD that reads the term, which is the granularity
            // `location` documents (`/States/<StateName>/<FieldName>`): a member
            // carrying several such fields is several problems to fix, each at its own
            // place in the caller's document, reported in the order the member writes
            // those fields.
            let reading = fields_reading(value, term, &nested_at(here, format_args!("/{shown}")), into.order());
            for (field, read) in reading {
                into.push(error(
                    UNSUPPORTED_JSONATA_EXPRESSION,
                    at(here, &format!("/{shown}{field}")),
                    field_does_not_exist(&read),
                ));
            }
        }
    }
}

/// One `Choice` rule of a JSONata state, and every rule nested in its `And`, `Or`
/// and `Not`: each one tests a `Condition`, never a JSONPath `Variable`.
///
/// `depth` bounds the nesting this follows, as every other expression tree here is
/// bounded: the rule tree is caller-shaped.
fn diagnose_jsonata_rule(
    name: &str,
    rule: &Value,
    here: &str,
    depth: usize,
    into: &mut Diagnosis,
) {
    let Some(depth) = depth.checked_sub(1) else {
        return;
    };
    let Some(rule) = rule.as_object() else {
        return;
    };
    for (member, value) in into.order().members(here, rule) {
        if member == "Variable" || member.ends_with(".$") {
            let shown = echoed(member);
            into.push(error(
                SCHEMA_VALIDATION_FAILED,
                at(here, &format!("/{shown}")),
                format!(
                    "a 'Choice' rule of state '{name}' is written in '{JSONATA}', so it must not \
                     carry the '{JSONPATH}' member '{shown}'"
                ),
            ));
        }
        match member.as_str() {
            CONDITION_MEMBER => {
                // "Condition — A JSONata expression that evaluates to true or false":
                // the member states the TEST the rule holds by, so it carries a boolean
                // or the expression that evaluates to one, judged by the same rule every
                // other member of a declared shape is ([`DeclaredShape`]).
                if let Some(why) = DeclaredShape::Boolean.unsatisfied(member, value) {
                    into.push(error(
                        SCHEMA_VALIDATION_FAILED,
                        at(here, &format!("/{}", echoed(member))),
                        why,
                    ));
                }
            }
            "And" | "Or" => {
                if let Some(nested) = value.as_array() {
                    for (index, rule) in nested.iter().enumerate() {
                        diagnose_jsonata_rule(
                            name,
                            rule,
                            &element_at(here, echoed(member), index),
                            depth,
                            into,
                        );
                    }
                }
            }
            "Not" => diagnose_jsonata_rule(name, value, &nested_at(here, "/Not"), depth, into),
            _ => {}
        }
    }
}

/// Every FIELD inside `value` whose text the language cannot PARSE as the expression it
/// is written as: the path from the member that carries `value` down to that field — the
/// empty path for a member that is itself such a field — paired with the parser's own
/// diagnostic for it ([`super::super::jsonata::parse_diagnostic`]).
///
/// A field is one the language PARSES when its whole value is one expression; a value
/// carrying anything else is the literal it is written as, so there is nothing to parse
/// in it. The list is in the order the definition writes those fields, read off
/// `into`'s order for the member at `member_at`, exactly as [`fields_reading`]'s is.
///
/// Each parse is spent from this walk's own budget ([`Diagnosis::spend_parse`]), so the
/// parsing one request asks for is bounded however many such fields the definition
/// declares.
fn unparsed_fields(value: &Value, member_at: &str, into: &mut Diagnosis) -> Vec<(String, String)> {
    let mut found = Vec::new();
    let mut parses_left = true;
    walk_fields(vec![(String::new(), value, false)], |path, text, _| {
        if !parses_left {
            return;
        }
        if !super::super::jsonata::is_expression(text) {
            return;
        }
        if !into.spend_parse() {
            parses_left = false;
            return;
        }
        if let Some(why) = super::super::jsonata::parse_diagnostic(text) {
            found.push((path.to_string(), why));
        }
    });
    found.sort();
    into.order().paths(member_at, &mut found);
    found
}

/// Every FIELD inside `value` whose JSONata expression reads `term`: the path from the
/// member that carries `value` down to that field — the empty path for a member that is
/// itself such an expression — paired with the field expression that read the term
/// there ([`field_read`]).
///
/// The list is in the order the definition writes those fields, read off `order` for
/// the member at `member_at`: a field the order does not carry falls after the ones it
/// does, in the lexicographic order that makes the per-field list of one member
/// deterministic whatever shape the document has.
fn fields_reading(
    value: &Value,
    term: &str,
    member_at: &str,
    order: &MemberOrder,
) -> Vec<(String, String)> {
    let mut found = Vec::new();
    walk_fields(vec![(String::new(), value, false)], |path, text, _| {
        // One field is one problem to fix, so it is reported once: the field
        // expression reported for it is the FIRST its expressions read the term
        // through.
        if let Some(read) = jsonata_expressions(text)
            .iter()
            .find_map(|expression| field_read(expression, term))
        {
            found.push((path.to_string(), read));
        }
    });
    found.sort();
    order.paths(member_at, &mut found);
    found
}

/// The SHAPE the Amazon States Language declares for the value of one member of a
/// JSONata state, of which it carries a value — or one JSONata expression, which
/// evaluates to one.
///
/// A member whose value may be any JSON value at all (a state's own `Output`) declares
/// no shape and carries none of these.
#[derive(Clone, Copy)]
enum DeclaredShape {
    /// A JSON OBJECT: the document the member builds.
    Document,
    /// A JSON ARRAY: the items the member holds.
    Array,
    /// A BOOLEAN: the test the member states.
    Boolean,
}

impl DeclaredShape {
    /// Why `value` does not satisfy this shape at the member `member`, or `None` when it
    /// does — and `None` too for the one defect reported on its own: a string that
    /// CARRIES the marks an expression is embedded between without being one is the
    /// malformed expression it is written as ([`malformed_expressions`]), reported there
    /// and not twice.
    ///
    /// The sentence a defect is stated by is the one its own value calls for. A STRING at
    /// such a member is only ever the expression it is written as — a string carrying no
    /// expression is a literal, which is of none of these shapes — so it is stated by the
    /// sentence this service states every value that is no expression by
    /// ([`malformed_expression_message`]); a value of any OTHER JSON type is judged by the
    /// SHAPE the language declares ([`Self::message`]).
    fn unsatisfied(self, member: &str, value: &Value) -> Option<String> {
        if let Some(text) = value.as_str() {
            if crate::state::jsonata::is_expression(text) || is_malformed_expression(text) {
                return None;
            }
            return Some(malformed_expression_message(&echoed(text)));
        }
        (!self.admits(value)).then(|| self.message(member))
    }

    /// Whether `value` is of this shape, or is the one form the language evaluates TO a
    /// value — a whole JSONata expression, whose own value no walk of the definition
    /// can know.
    ///
    /// A string is admitted ONLY as such an expression: a string carrying no expression
    /// is the literal it is written as, which is of none of these shapes. Whether the
    /// expression itself is well formed and parses is judged on its own
    /// ([`malformed_expressions`], [`unparsed_fields`]), so a value is not reported
    /// twice.
    fn admits(self, value: &Value) -> bool {
        if let Some(text) = value.as_str() {
            return crate::state::jsonata::is_expression(text);
        }
        match self {
            Self::Document => value.is_object(),
            Self::Array => value.is_array(),
            Self::Boolean => value.is_boolean(),
        }
    }

    /// What a schema diagnostic says about a member the definition wrote as something
    /// else: the member, and the two shapes the language declares for it.
    fn message(self, member: &str) -> String {
        let shape = match self {
            Self::Document => "a JSON object",
            Self::Array => "a JSON array",
            Self::Boolean => "a boolean",
        };
        format!(
            "'{}' must be {shape} or a JSONata expression",
            echoed(member)
        )
    }
}

/// The shape the language declares for the member `member` of a JSONata state, or
/// `None` for a member whose value it leaves open.
///
/// `Arguments` is the member a JSONata `Task`, `Map` or `Parallel` hands its work
/// ("Arguments […] used to pass information to the API actions of connected
/// resources") and `ItemSelector` the document a `Map` projects each item through, so
/// each builds a JSON object; `Items` is "the array that the Map state iterates over".
/// `Assign` is absent because its own rule ([`diagnose_assignments`]) is the stricter
/// one, and `Output` because "Output … can be any JSON value".
fn declared_member_shape(member: &str) -> Option<DeclaredShape> {
    match member {
        ARGUMENTS_MEMBER | ITEM_SELECTOR_MEMBER => Some(DeclaredShape::Document),
        ITEMS_MEMBER => Some(DeclaredShape::Array),
        _ => None,
    }
}

/// The member a JSONata `Map` state holds the array it iterates in: "Items".
const ITEMS_MEMBER: &str = "Items";

/// The member a JSONata `Choice` rule states its test in: "Condition".
const CONDITION_MEMBER: &str = "Condition";

/// Every FIELD inside `value` whose text CARRIES the marks a JSONata expression is
/// embedded between without BEING one such expression: the path from the member that
/// carries `value` down to that field, paired with the text itself.
///
/// The language embeds an expression as the WHOLE value of a field, opened with `{%`
/// and closed with `%}` — so text that opens one and does not close it, closes one it
/// never opened, carries anything outside the marks, or carries a second pair inside
/// them is a defect of the language's SCHEMA for that field and not the literal the
/// field would otherwise hold. It is told from [`unparsed_fields`], which is about the
/// expression INSIDE a well-formed pair.
///
/// The list is in the order the definition writes those fields, read off `order` for
/// the member at `member_at`, exactly as [`fields_reading`]'s is.
fn malformed_expressions(
    value: &Value,
    member_at: &str,
    order: &MemberOrder,
) -> Vec<(String, String)> {
    let mut found = Vec::new();
    walk_fields(vec![(String::new(), value, false)], |path, text, _| {
        if is_malformed_expression(text) {
            found.push((path.to_string(), echoed(text)));
        }
    });
    found.sort();
    order.paths(member_at, &mut found);
    found
}

/// Whether `text` carries the marks an embedded JSONata expression is delimited by
/// without being one whole expression between them.
fn is_malformed_expression(text: &str) -> bool {
    let text = text.trim();
    let opens = text.contains(EXPRESSION_OPEN);
    if !opens && !text.contains(EXPRESSION_CLOSE) {
        return false;
    }
    let Some(inner) = text
        .strip_prefix(EXPRESSION_OPEN)
        .and_then(|rest| rest.strip_suffix(EXPRESSION_CLOSE))
    else {
        return true;
    };
    inner.contains(EXPRESSION_OPEN) || inner.contains(EXPRESSION_CLOSE)
}

/// The marks the Amazon States Language embeds a JSONata expression between.
const EXPRESSION_OPEN: &str = "{%";
const EXPRESSION_CLOSE: &str = "%}";

/// What a diagnostic says about a value that is no embedded JSONata expression: the
/// text, quoted, and the form an embedded expression takes.
///
/// The one sentence every such value is stated by — the text that carries the marks
/// without being one expression ([`malformed_expressions`]) and the literal string
/// written at a member whose value the language evaluates
/// ([`DeclaredShape::unsatisfied`]) — so one rule is not worded two ways.
fn malformed_expression_message(text: &str) -> String {
    format!(
        "'{text}' is not a JSONata expression: an expression is the whole value of the field, \
         opened with '{EXPRESSION_OPEN}' and closed with '{EXPRESSION_CLOSE}'"
    )
}

/// Visit every string `roots` carries, with the path from its root down to it and
/// whether it is the value of a JSONPath reference field (a member whose name ends in
/// `.$`), which is what makes a string a reference rather than a literal.
///
/// The one walk of a caller-shaped document this module reads a state's fields with,
/// so the path a field is reported at cannot differ between what reads it. It is
/// ITERATIVE: the document is a caller's, so it carries its own stack rather than the
/// thread's, and each path is built through [`nested_at`], so it is bounded as it
/// grows.
fn walk_fields(roots: Vec<(String, &Value, bool)>, mut visit: impl FnMut(&str, &str, bool)) {
    let mut pending = roots;
    while let Some((path, value, reference_field)) = pending.pop() {
        match value {
            Value::String(text) => visit(&path, text, reference_field),
            Value::Array(items) => {
                for (index, item) in items.iter().enumerate() {
                    pending.push((indexed_at(&path, index), item, reference_field));
                }
            }
            Value::Object(members) => {
                for (member, child) in members {
                    pending.push((
                        nested_at(&path, format_args!("/{}", echoed(member))),
                        child,
                        member.ends_with(".$"),
                    ));
                }
            }
            _ => {}
        }
    }
}

/// The FIELD a JSONata `expression` reads `term` through, or `None` when it reads it
/// nowhere.
///
/// A field expression is the term together with the member accessors written after it:
/// the language reads a member of a value by a `.` followed by that member's name, so
/// the field runs from the term through every such step that follows it and ends at the
/// first character that can be part of neither. A term that is itself only the start of
/// a longer name is read to the end of that name, since what was written there is a
/// different field and not the term at all.
///
/// The result is [`echoed`]: the accessors are the caller's own text.
fn field_read(expression: &str, term: &str) -> Option<String> {
    let read = &expression[expression.find(term)?..];
    let bytes = read.as_bytes();
    let mut end = term.len();
    while bytes.get(end).is_some_and(|byte| is_member_byte(*byte)) {
        end += 1;
    }
    while bytes.get(end) == Some(&b'.') {
        let mut step = end + 1;
        let name = step;
        while bytes.get(step).is_some_and(|byte| is_member_byte(*byte)) {
            step += 1;
        }
        if step == name {
            break;
        }
        end = step;
    }
    Some(echoed(&read[..end]))
}

/// Whether `byte` can be part of a name written without quoting: what bounds the field
/// expression [`field_read`] reads, and the `$name` term
/// [`super::scan_expression`] reads.
pub(super) fn is_member_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// The payload templates a JSONPath state renders: every member of one is a LITERAL
/// unless its name ends in `.$`, which is what makes its value a path.
const PAYLOAD_TEMPLATE_MEMBERS: &[&str] = &["ItemSelector", "Parameters", "ResultSelector"];

/// Members whose string value names something — a state, a type, a language — rather
/// than holding the state's data, so a `$` in one is part of that name.
const NAMING_MEMBERS: &[&str] = &[
    "Comment",
    "Default",
    "Next",
    "QueryLanguage",
    "StartAt",
    "Type",
];

/// The WARNINGS a state written in JSONPath can carry — each one is a value that
/// READS as a path or an intrinsic invocation where the language takes a literal, so
/// the definition is created and the value is used verbatim — and the one ERROR about
/// the other direction: a field whose NAME makes its value a path, holding text that is
/// neither a path nor an intrinsic invocation ([`diagnose_reference_fields`]).
fn diagnose_jsonpath_state(
    name: &str,
    state: &Map<String, Value>,
    state_type: &str,
    here: &str,
    into: &mut Diagnosis,
) {
    // A `"<name>.$"` member is READ from the state's data, so what it holds has to be
    // something the language can read: the payload templates the state renders and the
    // variables it assigns are where such a member is written.
    for member in into.order().names(here, state) {
        if !PAYLOAD_TEMPLATE_MEMBERS.contains(&member.as_str()) && member != ASSIGN_MEMBER {
            continue;
        }
        if let Some(declared) = state.get(member) {
            diagnose_reference_fields(
                declared,
                &nested_at(here, format_args!("/{}", echoed(member))),
                into,
            );
        }
    }
    // PASS_RESULT_IS_STATIC: "Attempt to use a path in the result of a pass state".
    if state_type == "Pass" {
        if let Some(result) = state.get("Result").and_then(Value::as_str) {
            if looks_like_a_path(result) {
                into.push(warning(
                    PASS_RESULT_IS_STATIC,
                    at(here, "/Result"),
                    format!(
                        "the 'Result' of state '{name}' holds '{}', which reads as a path, \
                         but a 'Result' is the state's literal result: 'Parameters' or \
                         'ResultPath' read a path",
                        echoed(result)
                    ),
                ));
            }
        }
    }
    // NO_DOLLAR: "No `.$` on a field that appears to be a JSONPath or Intrinsic
    // Function".
    // NO_DOLLAR: "No `.$` on a field that appears to be a JSONPath or Intrinsic
    // Function". The state's own members are read in the order the definition writes
    // them, as everywhere here, so a state carrying two payload templates reports the
    // one it writes first first.
    for (member, declared) in into.order().members(here, state) {
        if !PAYLOAD_TEMPLATE_MEMBERS.contains(&member.as_str()) {
            continue;
        }
        let Some(template) = declared.as_object() else {
            continue;
        };
        let members = into
            .order()
            .members(&nested_at(here, format_args!("/{member}")), template);
        for (key, value) in members {
            let Some(text) = value.as_str() else {
                continue;
            };
            if key.ends_with(".$") || !(looks_like_a_path(text) || looks_like_an_intrinsic(text)) {
                continue;
            }
            let key = echoed(key);
            let text = echoed(text);
            into.push(warning(
                NO_DOLLAR,
                at(here, &format!("/{member}/{key}")),
                format!(
                    "'{key}' of the '{member}' of state '{name}' holds '{text}', which reads as a \
                     path or an intrinsic function, but a member evaluated as one is named \
                     '{key}.$'"
                ),
            ));
        }
    }
    // NO_PATH: "Field value looks like a path, but field name does not end with
    // 'Path'".
    for (member, value) in into.order().members(here, state) {
        let Some(text) = value.as_str() else {
            continue;
        };
        if NAMING_MEMBERS.contains(&member.as_str())
            || member.ends_with("Path")
            || member.ends_with(".$")
            || (state_type == "Pass" && member == "Result")
            || !looks_like_a_path(text)
        {
            continue;
        }
        let member = echoed(member);
        let text = echoed(text);
        into.push(warning(
            NO_PATH,
            at(here, &format!("/{member}")),
            format!(
                "the '{member}' of state '{name}' holds '{text}', which reads as a path, but \
                 '{member}' does not end in 'Path', so it is used as the literal it is"
            ),
        ));
    }
}

/// Whether `text` reads as a JSONPath: every path the language takes starts at the
/// state's data (`$`) or at the context object (`$$`).
fn looks_like_a_path(text: &str) -> bool {
    text.starts_with('$')
}

/// The REFERENCE-FIELD rule of one object a JSONPath state declares: a member whose
/// name ends in `.$` is READ from the state's data, so what it holds is a path or an
/// invocation of an intrinsic function and never a literal.
///
/// "To use a path in a payload template, the field name must end with `.$`" — the
/// suffix is how the language says the value is to be EVALUATED, so a member carrying
/// it and holding anything else (a plain string, a number, an object) asks for an
/// evaluation of something that is not an expression, which no run could perform. It is
/// the mirror of the [`NO_DOLLAR`] warning, and an ERROR for the reason that one is
/// not: there the field is used as the literal it is, here there is nothing to use.
///
/// The whole object is walked, so a reference field nested inside a payload template is
/// judged exactly as a top-level one is, with its own position reported; the walk is
/// ITERATIVE and drains its queue from the FRONT, so the fields are judged in the order
/// the definition writes them, and every path is built through [`nested_at`], like every
/// other walk here.
fn diagnose_reference_fields(declared: &Value, at_object: &str, into: &mut Diagnosis) {
    let mut pending: ::std::collections::VecDeque<(String, &Value)> =
        ::std::collections::VecDeque::from([(at_object.to_string(), declared)]);
    while let Some((path, value)) = pending.pop_front() {
        match value {
            Value::Object(members) => {
                for (member, value) in into.order().members(&path, members) {
                    let here = nested_at(&path, format_args!("/{}", echoed(member)));
                    let reads_a_path = member.ends_with(".$");
                    if reads_a_path && !value.as_str().is_some_and(reads_as_an_expression) {
                        into.push(error(
                            SCHEMA_VALIDATION_FAILED,
                            at(&here, ""),
                            reference_field_message(member),
                        ));
                        continue;
                    }
                    if !reads_a_path {
                        pending.push_back((here, value));
                    }
                }
            }
            Value::Array(items) => {
                for (index, item) in items.iter().enumerate() {
                    pending.push_back((indexed_at(&path, index), item));
                }
            }
            _ => {}
        }
    }
}

/// What a schema diagnostic says about such a field: the field, and the two things its
/// value may be.
///
/// RECORDED text. The field is the caller's own JSON key, so it is clipped by
/// [`echoed`] like every other caller value a diagnostic quotes.
fn reference_field_message(field: &str) -> String {
    format!(
        "The value for the field '{}' must be a valid JSONPath or intrinsic function expression",
        echoed(field)
    )
}

/// Whether `text` is written as something the language EVALUATES: a reference path, or
/// an invocation of an intrinsic function.
///
/// The two forms the language spells an evaluated value in, read off the text's
/// OPENING: a path starts at the state's data or at the context object
/// ([`looks_like_a_path`]), and every intrinsic the language defines is named
/// `States.<Name>`. Whether the invocation itself is well formed — its arguments
/// closed, its name one this service carries — is not decided here: that is what
/// evaluating it answers, and a definition carrying such an invocation is created and
/// fails at the state that evaluates it.
fn reads_as_an_expression(text: &str) -> bool {
    looks_like_a_path(text) || text.starts_with(INTRINSIC_PREFIX)
}

/// What every intrinsic function the Amazon States Language defines is named with.
const INTRINSIC_PREFIX: &str = "States.";

/// Whether `text` reads as an intrinsic-function invocation, which the language
/// spells `States.<Name>(<arguments>)`.
fn looks_like_an_intrinsic(text: &str) -> bool {
    text.starts_with("States.") && text.contains('(') && text.ends_with(')')
}

/// One transition a state declares: where it goes, where in the state the member that
/// names it sits, and WHICH of the state's assignment blocks runs on the way there.
pub(super) struct Transition {
    pub(super) target: String,
    /// The path from the state to the member naming the target.
    path: String,
    /// Which block of the state's assignments the language evaluates on the way along
    /// this transition.
    assigns: AssignedOnTheWay,
}

/// Which of a state's assignment blocks runs on the way along one of its transitions.
///
/// A state declares its assignments in three positions and the position decides WHEN
/// they run, so which of them a state the transition leads to can count on is a fact
/// about the transition rather than about the state: the state's own `Assign` runs when
/// its work COMPLETED, a `Catch` catcher's instead where the work failed and that
/// catcher took over, and a `Choice` rule's for the ONE rule that matched.
#[derive(Clone, Copy)]
enum AssignedOnTheWay {
    /// The state's own `Assign`.
    Own,
    /// The `Assign` of the `Catch` catcher at that index.
    Catcher(usize),
    /// The `Assign` of the `Choice` rule at that index.
    ChoiceRule(usize),
}

/// Every transition `state` declares, in the order the members that name them are
/// judged: its own `Next`, the `Default` of a `Choice`, each `Choice` rule's own
/// `Next`, and each `Catch` catcher's `Next`.
///
/// ONE enumeration, read both by the diagnostic that resolves a target against the
/// enclosing `States` map and by the state graph the variable-scope analysis walks, so
/// the two cannot disagree about where a state can go.
///
/// Linear in the state's own size, which the modelled `Definition` (1 MiB) bounds, so
/// the list this builds is bounded by the request exactly as the state is.
pub(super) fn transitions(state: &Map<String, Value>) -> Vec<Transition> {
    let mut transitions = Vec::new();
    for member in ["Next", "Default"] {
        if let Some(target) = state.get(member).and_then(Value::as_str) {
            transitions.push(Transition {
                target: target.to_string(),
                path: format!("/{member}"),
                assigns: AssignedOnTheWay::Own,
            });
        }
    }
    for (member, from_choices) in [(CHOICES_MEMBER, true), (CATCH_MEMBER, false)] {
        let entries = state
            .get(member)
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        for (index, entry) in entries.iter().enumerate() {
            if let Some(target) = entry.get("Next").and_then(Value::as_str) {
                transitions.push(Transition {
                    target: target.to_string(),
                    path: nested_at(&element_at("", member, index), "/Next"),
                    assigns: match from_choices {
                        true => AssignedOnTheWay::ChoiceRule(index),
                        false => AssignedOnTheWay::Catcher(index),
                    },
                });
            }
        }
    }
    transitions
}

/// The states `state` can transition to, however it names them: what the lone state
/// of a [`super::validate_single_state`] has declared beside it.
pub(super) fn transition_targets(state: &Value) -> Vec<String> {
    state
        .as_object()
        .map(|state| {
            transitions(state)
                .into_iter()
                .map(|transition| transition.target)
                .collect()
        })
        .unwrap_or_default()
}


