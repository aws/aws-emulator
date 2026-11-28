// SPDX-License-Identifier: CC0-1.0

//! The violations of ONE request that the framework's validating walk does not report, read
//! off the request body.
//!
//! That walk finds every bound the model declares — a `@length`, a `@range`, an enumerated
//! member's value set, an absent `@required` member — and hands them to
//! [`super::EmulatedService::override_refusal`] as violations, which that seam words as one
//! clause per violated member inside the `<N> validation error(s) detected: ` envelope. A
//! request can break a constraint that walk never reports in two ways, and a refusal that
//! reported only what the walk found would state one clause where AWS states several, with
//! a count a client reads violations off that is short:
//!
//! 1. the constraint is one the model does NOT declare, so no walk could find it — the
//!    first half below, which reads the request and words the clause AWS states for it;
//! 2. the constraint IS declared, but the walk never ran: the framework deserializes the
//!    body before it walks the built input, and a member the input requires and the body
//!    omits stops the first step — the second half below, which reads those bounds off the
//!    generated input SCHEMA and states the clauses that walk would have.
//!
//! Both halves read the request BODY, because that is all either is about — no stored state
//! decides any of them — and this is the one layer that has the body in hand before the
//! operation runs ([`super::OperationScopedClauseForm`]).
//!
//! A clause of the FIRST half keeps the wording AWS states for it rather than the form the
//! served operation states the WALK's clauses in (`super`'s "Which form a constraint
//! clause takes"): the walk is not what found it, so there is no declared bound to render
//! from — exactly as an enumerated member's value set and an absent member keep the
//! reader's own rendering whichever operation was served. A violation of the SECOND half is
//! the walk's own, so it is handed to that renderer and takes the served operation's form
//! like any other.

use serde_json::Value;

use aws_smithy_runtime_api::http::Request as RtRequest;
use aws_smithy_schema_server::constraints::{EnumTrait, LengthTrait, Violation, ViolationKind};
use smithy_server_runtime::{DeserializableStruct, ServerHttpProtocol};

/// The operation whose elements must each state one of a set of members the model leaves
/// all four optional, and the member list they arrive in.
const TRANSACT_WRITE_ITEMS: &str = "TransactWriteItems";

/// The list member that carries those elements, as the request spells it on the wire.
const TRANSACT_ITEMS_MEMBER: &str = "TransactItems";

/// The four members one element may state, as the model spells them
/// (`smithy_query(query="shape", shape="com.amazonaws.dynamodb#TransactWriteItem")`).
const TRANSACT_ITEM_ACTIONS: [&str; 4] = ["ConditionCheck", "Put", "Delete", "Update"];

/// The member a table NAME arrives in, as the model spells it on the wire.
const TABLE_NAME_MEMBER: &str = "TableName";

/// The clauses the `TableName` of a request that NAMES the table it creates earns, read off
/// `req`'s body.
///
/// Empty for an operation that does not hold the member to the table-name form
/// (`super::TABLE_NAME_FORM_OPERATIONS`), for a request that carries no name, and for a
/// name that takes the form. The clauses are read HERE, off the body, because they answer
/// for a refusal the validating WALK may have raised about ANY member: that walk reports
/// the bound the model declares on a member that admits a table's ARN as well as its name,
/// which is wider than the form a NAME takes, and the clauses a caller reads are this
/// form's — so the whole refusal is stated from the name, and the handler's own check words
/// the identical clauses for the request that reaches it
/// (`crate::state::table_name_form_clauses`).
pub fn table_name_form_clauses(operation: &str, req: &RtRequest) -> Vec<String> {
    if !super::holds_table_name_to_its_form(operation) {
        return Vec::new();
    }
    let Some(body) = req.body().bytes() else {
        return Vec::new();
    };
    let Ok(document) = ::serde_json::from_slice::<Value>(body) else {
        return Vec::new();
    };
    let Some(name) = document.get(TABLE_NAME_MEMBER).and_then(Value::as_str) else {
        // A member the request does not carry is the absent member this service's own
        // parameter check states, which is a different refusal.
        return Vec::new();
    };
    crate::state::table_name_form_clauses(crate::state::TABLE_NAME_MEMBER_PATH, name)
}

/// The clauses this service's own reading of `req` found for the operation it is being
/// served as, in the order the input declares the members they are about — so they join
/// the walk's clauses in the one order the refusal states its members in.
///
/// Empty for every request that violates none of them, which is every request of every
/// operation that has no such constraint.
pub fn service_checked_clauses(operation: &str, req: &RtRequest) -> Vec<String> {
    if operation != TRANSACT_WRITE_ITEMS {
        return Vec::new();
    }
    let Some(body) = req.body().bytes() else {
        return Vec::new();
    };
    let Ok(document) = ::serde_json::from_slice::<Value>(body) else {
        return Vec::new();
    };
    transact_items_clauses(&document)
}

/// The clause a `TransactWriteItems` body earns when one of its elements states none of
/// the four actions.
///
/// ONE clause however many elements state none: the clause names the LIST member, which
/// is violated once — the path AWS reports it under names no element position.
///
/// Only the element that states NONE of them is read here. An element that states SEVERAL
/// states no single action either, and that one is refused by the handler, which has the
/// decoded actions in hand and words it as that defect
/// (`crate::state::exactly_one_operation`, reached from `Requested::of`): it is not read
/// off the body, because a body carrying it reaches the handler — nothing the validating
/// walk finds can mask it.
fn transact_items_clauses(document: &Value) -> Vec<String> {
    let Some(elements) = document.get(TRANSACT_ITEMS_MEMBER).and_then(Value::as_array) else {
        // No list at all is the absent `@required` member the walk reports, and a member
        // that is not a list does not decode into the operation's input at all.
        return Vec::new();
    };
    let states_no_action = elements.iter().any(|element| {
        element.as_object().is_some_and(|members| {
            TRANSACT_ITEM_ACTIONS
                .iter()
                .all(|action| members.get(*action).is_none_or(Value::is_null))
        })
    });
    if states_no_action {
        return vec![crate::state::transact_item_states_no_action_clause()];
    }
    Vec::new()
}

// =============================================================================
// A CAPPED LIST PAST ITS MAXIMUM
//
// A list member carrying more elements than the model admits is a violation the validating
// WALK reports, so the request is refused before the handler that has the decoded list in
// hand runs, and the clause states what every other violation of a declared bound states:
// the member path and the bound, in the form the served operation states its clauses in
// (`super`'s "Which form a constraint clause takes"). For most such members nothing of the
// rejected list belongs in it — the clause a transactional READ's list past its 100-action
// cap earns names `TransactItems` and the bound and echoes NOTHING, exactly as the same
// member's EMPTY case does (`tests/all/collection_cap_constraint_reports.rs` pins both, and
// `tests/all/transact_items_cap_refusal.rs` drives the maximum over the two transaction
// operations).
//
// The recorded refusals of the two lists a WRITE states its actions in state the REJECTED
// LIST instead: the PartiQL transaction's `TransactStatements`, quoted as
// `Value '[ParameterizedStatement(statement=…, …)]' at 'transactStatements' …`, and the
// transactional write's `TransactItems`, quoted as
// `Value '[TransactWriteItem(conditionCheck=null, put=Put(item={…}, …), delete=null,
// update=null), …]' at 'TransactItems' …` — the same object rendering a refused
// `BatchWriteItem` request map is quoted in. The walk hands
// [`super::EmulatedService::override_refusal`] its own summary of the collection (`<n>
// items`) rather than the collection, so there is nothing there to render from.
//
// That list is therefore read HERE, where the request still is: decoded by the framework's
// OWN deserializer for the served operation's input — the same protocol, the same input
// schema and the same generated reader the dispatch that follows uses, so this layer
// cannot disagree with it — and rendered by the one renderer this crate states that form
// with (`crate::state::rendered_parameterized_statements`). The BOUND is read off the same
// generated input schema the walk reads it off, never off a second transcription of the
// model.
//
// Only a list past the MAXIMUM is stated this way, which is the class those recorded
// refusals carry. A list below the member's minimum is the empty list, whose recorded
// refusal states the member and the bound alone, so it keeps the walk's own clause.
// =============================================================================

/// The PartiQL transaction, whose own statement list is quoted back that way.
const EXECUTE_TRANSACTION: &str = "ExecuteTransaction";

/// The list member of `ExecuteTransaction`, as the model spells it on the wire.
const TRANSACT_STATEMENTS_MEMBER: &str = "TransactStatements";

/// The clause one capped list earns, beside the member path the walk reports that list
/// under — so the refusal substitutes it for the walk's own clause about THAT member and
/// leaves every other clause of the same refusal alone.
#[derive(Debug, Clone)]
pub struct CappedListClause {
    /// The member path the validating walk reports the violated list under.
    pub path: &'static str,
    /// The clause this service words for it, with the rejected list rendered from the
    /// request.
    pub clause: String,
}

/// The clause the served operation's capped list earns for carrying more elements than the
/// model admits, with the rejected list rendered from the request itself.
///
/// `None` for an operation that has no such list, for a body no input can be read from, and
/// for a list inside its declared bound — including an empty one, which keeps the walk's
/// clause.
pub fn capped_list_clause(
    operation: &str,
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Option<CappedListClause> {
    let (path, rendered, requirement) = match operation {
        EXECUTE_TRANSACTION => {
            let schema = crate::ops::types::EXECUTE_TRANSACTION_INPUT_SCHEMA;
            let input: crate::ops::types::ExecuteTransactionInput = decoded(protocol, req, schema)?;
            let requirement = past_declared_maximum(
                schema,
                TRANSACT_STATEMENTS_MEMBER,
                input.transact_statements.len(),
            )?;
            (
                crate::state::TRANSACT_STATEMENTS_MEMBER_PATH,
                crate::state::rendered_parameterized_statements(&input.transact_statements),
                requirement,
            )
        }
        // The transactional WRITE's own action list, quoted the same way: the service's
        // object rendering of the actions the request carried
        // ([`crate::state::rendered_transact_write_items`]), under the path the walk reports
        // that member of this operation under — which is the one the served operation states
        // its members by ([`super::CLAUSE_FORMS`] gives this surface the wire spelling).
        TRANSACT_WRITE_ITEMS => {
            let schema = crate::ops::types::TRANSACT_WRITE_ITEMS_INPUT_SCHEMA;
            let input: crate::ops::types::TransactWriteItemsInput = decoded(protocol, req, schema)?;
            let requirement =
                past_declared_maximum(schema, TRANSACT_ITEMS_MEMBER, input.transact_items.len())?;
            (
                crate::state::TRANSACT_ITEMS_MEMBER_PATH,
                crate::state::rendered_transact_write_items(&input.transact_items),
                requirement,
            )
        }
        _ => return None,
    };
    // The member is NAMED the way the served operation names its members
    // ([`super::ClauseForms`]) — the same path the walk's own clause about it would state —
    // and what this seam adds is the VALUE half, which that form has nothing to render from.
    let stated = super::clause_forms(operation).of(path).stated_path(path);
    Some(CappedListClause {
        path,
        clause: crate::state::constraint_clause(&stated, Some(&rendered), &requirement),
    })
}

/// `req`'s body read as the input `schema` declares, by the framework's own deserializer
/// for it — `None` for a body no input can be read from, which is a refusal of its own
/// (`super::request_layer`).
fn decoded<I: DeserializableStruct>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
    schema: &'static ::aws_smithy_schema::Schema,
) -> Option<I> {
    let mut deserializer = protocol.deserialize_input(req, schema).ok()?;
    I::deserialize(&mut *deserializer).ok()
}

/// The requirement phrase for the `@length` MAXIMUM `member` of `input` declares, when
/// `elements` is past it — read off the generated schema the validating walk reads it off.
/// `None` when the member declares no maximum, and when the list is inside it.
fn past_declared_maximum(
    input: &'static ::aws_smithy_schema::Schema,
    member: &str,
    elements: usize,
) -> Option<String> {
    let declared_member = input
        .members()
        .iter()
        .find(|declared| declared.member_name() == Some(member))?;
    let declared =
        declared_trait::<LengthTrait>(declared_member.traits()?, &LengthTrait::TRAIT_ID)?;
    let max = declared.max?;
    let length = i64::try_from(elements).unwrap_or(i64::MAX);
    (length > max).then(|| crate::state::max_length_requirement(usize::try_from(max).unwrap_or(0)))
}

// =============================================================================
// THE BOUNDS THE VALIDATING WALK NEVER GOT TO
//
// The framework reads a request in two steps: it DESERIALIZES the body into the
// operation's input, and then walks that input for every bound the model declares. A
// member the input REQUIRES and the body does not carry is found by the first step, which
// cannot complete — so the walk never runs, and every other bound the same body breaks is
// found by nobody. One request, several independent violations, one of them reported.
//
// AWS reports them together: a request breaking four of its input's constraints is refused
// with four clauses in one counted envelope, so a client fixing a malformed request sees
// every defect of it rather than one per round trip. That ORDER — deserialize, then walk —
// is the vendored framework's (`vendor/smithy-server-runtime`'s operation sequence, which
// this tree does not author), so the bounds that walk did not get to are read HERE off the
// body instead.
//
// They are read off the SAME generated input schema the walk reads them off
// (`crate::ops::types`'s `*_INPUT_SCHEMA`, where codegen hangs each member's `@length`,
// `@pattern` and value set), never off a second transcription of the model: a bound the
// model changes moves in both places at once, and the clause a defect earns here is the
// clause that walk would have stated — the member path, the rejected value and the
// requirement — so it is WORDED by the one renderer that words the walk's clauses
// (`super::violation_clause`) and counted in the one envelope beside them
// (`super::validation_envelope`).
//
// What is read is the TOP-LEVEL STRING members of the served input: those are the members
// a body carrying an absent required member can still break a declared bound of. The
// violations are produced for every such request and used only by a refusal that reports
// an absent required member, which is the one refusal whose walk never ran.
// =============================================================================

/// The input schema of the operation whose body is read for those bounds, or `None` for an
/// operation this reads nothing of.
///
/// The operations named are the ones whose input declares a required member BESIDES the
/// table name: a request that omits the table name is refused by this service's own
/// parameter check, which states its one sentence and nothing else
/// (`super::SERVICE_REQUIRED_MEMBERS`), so the walk has nothing to be masked by there. An
/// operation later found to mask a violation this way needs its own arm — the schema is all
/// an arm carries, so no bound, no value set and no member name is written out here.
/// `tests/all/request_shape_validation_classes.rs` reads the aggregate back over the
/// operations that carry one.
fn body_read_input(operation: &str) -> Option<&'static ::aws_smithy_schema::Schema> {
    Some(match operation {
        "DeleteItem" => crate::ops::types::DELETE_ITEM_INPUT_SCHEMA,
        "GetItem" => crate::ops::types::GET_ITEM_INPUT_SCHEMA,
        "PutItem" => crate::ops::types::PUT_ITEM_INPUT_SCHEMA,
        "UpdateItem" => crate::ops::types::UPDATE_ITEM_INPUT_SCHEMA,
        _ => return None,
    })
}

/// The violations of `operation`'s declared bounds this service can still read off `req`'s
/// body, kept for the one refusal that needs them.
///
/// `None` for every operation whose body this reads nothing of. The body is COPIED and not
/// read here: the refusal these answer for is the one whose deserialization aborted, so
/// reading the body eagerly would make every well-formed request of these operations pay a
/// second full parse of an item it already parsed once.
pub fn readable_body(operation: &str, req: &RtRequest) -> Option<ReadableBody> {
    let input = body_read_input(operation)?;
    Some(ReadableBody {
        input,
        body: req.body().bytes()?.to_vec(),
    })
}

/// One request's body beside the schema its bounds are declared in.
#[derive(Debug)]
pub struct ReadableBody {
    /// The generated schema of the operation's input, where codegen hung each member's
    /// declared bounds.
    input: &'static ::aws_smithy_schema::Schema,
    /// The body as it arrived.
    body: Vec<u8>,
}

impl ReadableBody {
    /// The violations of the input's declared bounds this body breaks.
    ///
    /// Empty for every request that breaks none of them. The order several of them are
    /// stated in is the refusal's ([`super::in_stated_member_order`]), like the walk's own.
    pub fn violations(&self) -> Vec<Violation> {
        let Ok(document) = ::serde_json::from_slice::<Value>(&self.body) else {
            return Vec::new();
        };
        self.input
            .members()
            .iter()
            .flat_map(|member| broken_bounds(member, &document))
            .collect()
    }
}

/// Every bound `member` declares that the value the body carries for it breaks.
///
/// The member is read by the name the model spells it under, which is the name this
/// protocol puts on the wire (`awsJson` reads a struct member by its member name), and only
/// a STRING value is read: every bound below is a string member's, and a member carrying
/// another datatype does not decode into the input at all, which is a different refusal.
fn broken_bounds(member: &::aws_smithy_schema::Schema, document: &Value) -> Vec<Violation> {
    let Some(traits) = member.traits() else {
        return Vec::new();
    };
    let Some(value) = member
        .member_name()
        .and_then(|name| document.get(name))
        .and_then(Value::as_str)
    else {
        return Vec::new();
    };
    // In the order the walk runs a leaf's traits, so a member breaking two of its bounds
    // earns the same clauses in the same order whichever reader found them. A `@pattern` is
    // not among them: no top-level member of the inputs above declares one, and compiling a
    // declared expression is the validating walk's own business — it holds the one compiled
    // pattern cache in the tree.
    let kinds = broken_length(traits, value)
        .into_iter()
        .chain(broken_value_set(traits, value));
    kinds
        .map(|kind| Violation {
            // The path the validating walk reports a top-level member under.
            path: ::aws_smithy_schema_server::constraints::path::member(
                member.member_name().unwrap_or_default(),
            ),
            kind,
            // Held to the same ceiling the walk holds its echoed values to, so a caller
            // cannot amplify a request into an arbitrarily long response
            // ([`crate::state::MAX_ECHOED_CLAUSE_VALUE`]).
            value: Some(crate::state::echoed_clause_value(value).into_owned()),
            sensitive: false,
        })
        .collect()
}

/// The `@length` bound `value` breaks, counted in CHARACTERS — the length the walk counts
/// for a string.
fn broken_length(traits: &::aws_smithy_schema::TraitMap, value: &str) -> Option<ViolationKind> {
    let declared = declared_trait::<LengthTrait>(traits, &LengthTrait::TRAIT_ID)?;
    let length = i64::try_from(value.chars().count()).unwrap_or(i64::MAX);
    match (declared.min, declared.max) {
        (Some(min), _) if length < min => Some(ViolationKind::MinLength(min)),
        (_, Some(max)) if length > max => Some(ViolationKind::MaxLength(max)),
        _ => None,
    }
}

/// The value set `value` falls outside, in the order the model declares it: the set a
/// clause STATES is then the renderer's business (`super::stated_enum_order`), exactly as
/// for the walk's own clause.
fn broken_value_set(traits: &::aws_smithy_schema::TraitMap, value: &str) -> Option<ViolationKind> {
    let declared = declared_trait::<EnumTrait>(traits, &EnumTrait::TRAIT_ID)?;
    let admitted = declared.values.iter().any(|admissible| {
        if declared.ignore_case {
            admissible.eq_ignore_ascii_case(value)
        } else {
            *admissible == value
        }
    });
    (!admitted).then(|| {
        ViolationKind::EnumValue(declared.values.iter().map(|v| (*v).to_string()).collect())
    })
}

/// One constraint trait of a member schema, as codegen hung it there.
fn declared_trait<'t, T: 'static>(
    traits: &'t ::aws_smithy_schema::TraitMap,
    id: &::aws_smithy_schema::ShapeId,
) -> Option<&'t T> {
    traits.get(id)?.as_any().downcast_ref::<T>()
}

/// Where the reader of `req`'s body stands once it has read the request object whole: the
/// one-based line and column serde's own reader fixes to a failure raised inside that
/// read, which is the position a `missing field` refusal states.
///
/// `None` for an operation whose reader states no position for an absent member
/// (`super::BODY_READER_OPERATIONS`), and for bytes that are not text at all — bytes no
/// reader gets a position in are refused as the protocol's own marshalling failure instead
/// (`super::request_layer`).
///
/// A read only runs out of members once the object it was building has been consumed
/// whole, so the position such a refusal states is that object's end rather than the
/// member's. Nothing is scanned for it: the position comes from the reader that ran, which
/// is why a brace inside a string value, an escape and a nested object need no handling
/// here.
pub fn body_read_end(operation: &str, req: &RtRequest) -> Option<(usize, usize)> {
    if !super::reads_body_with_its_own_reader(operation) {
        return None;
    }
    let body = ::std::str::from_utf8(req.body().bytes()?).ok()?;
    Some(read_to_the_end_of_the_object(body))
}

/// Read `body` as an object, consuming every entry it carries, and report the position the
/// reader fixes to the failure that read then raises.
///
/// The failure IS the reading: `serde_json` fixes the position of an error raised while
/// reading an object to just past that object's close, so running a read that consumes the
/// object and then reports it ran out of members is what produces the position such a
/// refusal states — the same reader, the same rule, with nothing re-scanning the document.
/// A body whose top-level value is not an object is reported at the position that value's
/// own read failed at; no member of such a body is ever found absent, because a body that
/// builds no input is refused as a marshalling failure first.
fn read_to_the_end_of_the_object(body: &str) -> (usize, usize) {
    let mut reader = ::serde_json::Deserializer::from_str(body);
    match ::serde::Deserializer::deserialize_map(&mut reader, RanOutOfMembers) {
        Err(reported) => (reported.line(), reported.column()),
        // The reader above reports a failure for every body, so there is no value here:
        // `Infallible` has no variants to name.
        Ok(unreachable) => match unreachable {},
    }
}

/// A reader that consumes a JSON object whole and then reports that it ran out of members
/// — the read a refusal naming an absent member is the position of.
struct RanOutOfMembers;

impl<'de> ::serde::de::Visitor<'de> for RanOutOfMembers {
    type Value = ::std::convert::Infallible;

    fn expecting(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter.write_str("a JSON object")
    }

    fn visit_map<A: ::serde::de::MapAccess<'de>>(
        self,
        mut entries: A,
    ) -> Result<Self::Value, A::Error> {
        // What the entries HOLD is read by the layers that are about the values; this read
        // is about where the object ends, so each entry is stepped over.
        while entries
            .next_entry::<::serde::de::IgnoredAny, ::serde::de::IgnoredAny>()?
            .is_some()
        {}
        Err(::serde::de::Error::custom("the object ends here"))
    }
}

#[cfg(test)]
mod tests {
    use super::read_to_the_end_of_the_object;

    #[test]
    fn the_read_ends_past_everything_nested_and_quoted_in_the_object() {
        // Each row is a body and the position the read of its object ends at: a flat
        // object, one with a nested object, a brace inside a string value, an escaped
        // quote before such a brace, and a body with bytes after the object.
        let rows: Vec<(&str, (usize, usize))> = vec![
            ("{}", (1, 2)),
            (r#"{"a":1}"#, (1, 7)),
            (r#"{"a":{"b":1}}"#, (1, 13)),
            (r#"{"a":"}"}"#, (1, 9)),
            (r#"{"a":"\"}"}"#, (1, 11)),
            (r#"{"a":1}  "#, (1, 7)),
        ];
        for (body, expected) in rows {
            let found = read_to_the_end_of_the_object(body);
            assert_eq!(
                found, expected,
                "case {body:?}: expected the read to end at {expected:?}, got {found:?}"
            );
        }
    }

    #[test]
    fn a_column_counts_from_the_start_of_the_line_the_read_ended_on() {
        // A body written over several lines: the line the object closes on, and the bytes
        // consumed on that line — which is how the reader counts a position.
        let rows: Vec<(&str, (usize, usize))> = vec![
            ("{\n  \"a\": 1\n}", (3, 1)),
            ("{\n  \"a\": {\n    \"b\": 1\n  }\n}", (5, 1)),
            ("{\"a\": 1,\n \"b\": 2}", (2, 8)),
        ];
        for (body, expected) in rows {
            let found = read_to_the_end_of_the_object(body);
            assert_eq!(
                found, expected,
                "case {body:?}: expected the read to end at {expected:?}, got {found:?}"
            );
        }
    }

    #[test]
    fn a_body_that_never_closes_its_object_is_reported_where_the_read_stopped() {
        // Bytes a reader cannot finish: the reader still states where it got to, which is
        // the end of the input it ran out of.
        for body in [r#"{"a":1"#, ""] {
            let (line, column) = read_to_the_end_of_the_object(body);
            assert_eq!(
                line, 1,
                "case {body:?}: expected the first line, got line {line} column {column}"
            );
        }
    }
}


