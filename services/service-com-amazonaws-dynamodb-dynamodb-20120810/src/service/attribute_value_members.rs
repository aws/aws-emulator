// SPDX-License-Identifier: CC0-1.0

//! The refusal of an `AttributeValue` a request sends with NO datatype — or with several, or
//! with a `null` standing where a SET member stands — stated behind the member it stands in.
//!
//! What is wrong with such a value is one fact, worded once
//! ([`EMPTY_ATTRIBUTE_VALUE_MESSAGE`] and its ambiguous twin), and most
//! members state that fact alone. A member whose value is read as ONE THING rather than as a
//! value in its own right states it behind the sentence for that thing — a resume token that
//! carries an unreadable value is the invalid token it is
//! ([`crate::state::START_KEY_INVALID_MESSAGE`]), with the datatype fact as the reason — and a
//! member that is a TABLE of entries states it as that table carrying an invalid value under
//! the key the value sits at.
//!
//! It is read HERE, off the request body, because a value the codec cannot read never becomes
//! an input: the request is refused while it is being deserialized, before any handler could
//! say which member the value stood in — the codec's error names the union and not the rule the
//! value broke (`vendor/smithy-macros` reports "union 'AttributeValue' had no member set"), and
//! a value carrying SEVERAL datatypes is not a failure of that reader at all: it reads the last
//! one and serves a request AWS refuses. So the body is read ahead of the dispatch, in the one
//! scope every request this service serves already passes through
//! ([`super::OperationScopedClauseForm`]), over every position an `AttributeValue` stands in
//! ([`VALUE_BEARING_MEMBERS`]).
//!
//! Nothing about the model distinguishes the two WORDINGS — the same union, the same defect, a
//! different sentence — so [`ENVELOPED_VALUE_MEMBERS`] is the only place the difference is
//! recorded, and a member later found to state one needs its own entry there.

use aws_smithy_runtime_api::http::Request as RtRequest;
use serde_json::{Map, Value};

/// AWS's fixed text for an `AttributeValue` that carries NO datatype at all: the union
/// declares ten of them and exactly one must be set
/// (`smithy_query(query="shape", shape="com.amazonaws.dynamodb#AttributeValue")`), so a value
/// naming none carries no value at all. Fixed text with no variable part — the sentence names
/// the API concept and not the member the value stood in, which the clause around it states.
///
/// Stated HERE, beside the one walk that reports it, rather than among the types rendered from
/// the model: a wording is this emulator's own and a rendered file is rewritten by the next
/// model update.
const EMPTY_ATTRIBUTE_VALUE_MESSAGE: &str =
    "Supplied AttributeValue is empty, must contain exactly one of the supported datatypes";

/// AWS's fixed text for an `AttributeValue` that carries MORE THAN ONE datatype: the same
/// constraint stated for the other way of breaking it, and worded here for the same reason as
/// its empty twin above.
const AMBIGUOUS_ATTRIBUTE_VALUE_MESSAGE: &str =
    "Supplied AttributeValue has more than one datatypes set, must contain exactly one of the \
     supported datatypes";

/// The datatypes one `AttributeValue` may carry, as the model spells them on the wire —
/// exactly the ten `smithy_query(query="shape", shape="com.amazonaws.dynamodb#AttributeValue")`
/// declares (`S N B SS NS BS M L NULL BOOL`), which is the set the reader of that union
/// recognises (`crate::ops::types::AttributeValue`). A member name outside it is not a
/// datatype at all and is skipped by that reader, so it is skipped here too.
const DATATYPES: [&str; 10] = ["S", "N", "B", "SS", "NS", "BS", "M", "L", "NULL", "BOOL"];

/// The two datatypes a value NESTS through, which is why the walk below recurses: the values
/// inside them are `AttributeValue`s read by the same reader, so a defect inside one is the
/// same defect.
const NESTED_MAP: &str = "M";
const NESTED_LIST: &str = "L";

/// The members whose values this service states a datatype defect BEHIND a sentence of their
/// own, each with the FORM it states it in.
///
/// A resume token is read as a whole — a caller gets it from the previous page's
/// `LastEvaluatedKey` rather than composing it — so a token this service cannot read a value
/// out of is the invalid token it is ([`Stated::Behind`]), with the datatype fact as the
/// reason and no entry of it named. A substitution table is a table of entries a caller wrote
/// one at a time, so a value it cannot substitute is reported as that table carrying an
/// invalid value under the KEY it sits at ([`Stated::UnderKey`]) — the same wording the
/// NAMES table's own invalid entry is reported under
/// (`crate::state`'s `check_tables_not_empty`).
const ENVELOPED_VALUE_MEMBERS: &[(&str, Stated)] = &[
    (
        "ExclusiveStartKey",
        Stated::Behind(crate::state::START_KEY_INVALID_MESSAGE),
    ),
    ("ExpressionAttributeValues", Stated::UnderKey),
];

/// How one member states a datatype defect of a value it carries.
#[derive(Debug, Clone, Copy)]
enum Stated {
    /// Behind a sentence about that member's value as a WHOLE, which names no entry of it.
    Behind(&'static str),
    /// As the member carrying an invalid value, named by the KEY the value sits under.
    UnderKey,
}

impl Stated {
    /// The whole sentence for `defect`, found under `key` of `member`.
    ///
    /// The key is the CALLER's text, so it is echoed held to the ceiling every clause of this
    /// service quotes a rejected value at ([`crate::state::echoed_clause_value`]): this layer
    /// answers before the handler that holds a substitution key to its own length, so the
    /// sentence cannot be sized by the request.
    fn sentence(self, member: &str, key: &str, defect: &str) -> String {
        match self {
            Self::Behind(stated) => format!("{stated}: {defect}"),
            Self::UnderKey => {
                let echoed = crate::state::echoed_clause_value(key);
                format!("{member} contains invalid value: {defect} for key {echoed}")
            }
        }
    }
}

/// The operations whose input declares one of the members above AT THE REQUEST LEVEL, sorted.
///
/// The gate on reading the body at all: a request of any other operation carries no such
/// member, so no walk over its body could answer anything, and the body of an item write or a
/// batch — the largest this service serves — is never parsed a second time for this layer.
/// Read off the model: `ExclusiveStartKey` is declared by the two table walks
/// (`smithy_query(query="operation", operation="Query,Scan")`) and `ExpressionAttributeValues`
/// by those two plus the three single-item writes
/// (`smithy_query(query="operation", operation="PutItem,UpdateItem,DeleteItem")`). The same
/// members nested inside a `TransactItems` or `RequestItems` ELEMENT belong to that element
/// rather than to the request and keep the codec's own refusal.
const MEMBER_BEARING_OPERATIONS: [&str; 5] =
    ["DeleteItem", "PutItem", "Query", "Scan", "UpdateItem"];

/// The sentence refusing `req`'s body for an `AttributeValue` with no datatype, or with
/// several, standing under one of the members above — or `None` when no such member carries
/// one, which is every request of every operation that carries none of them.
///
/// The answer is the first defect found, over the members in the order above and the
/// attributes of one member in the order the body's reader reports them, so one body earns one
/// sentence rather than a sentence that depends on the order a map happened to be walked in.
pub fn enveloped_refusal(operation: &str, req: &RtRequest) -> Option<String> {
    if MEMBER_BEARING_OPERATIONS.binary_search(&operation).is_err() {
        return None;
    }
    let body = req.body().bytes()?;
    let document = ::serde_json::from_slice::<Value>(body).ok()?;
    let members = document.as_object()?;
    ENVELOPED_VALUE_MEMBERS.iter().find_map(|(member, stated)| {
        let carried = members.get(*member)?.as_object()?;
        let (key, defect) = first_defect(carried)?;
        Some(stated.sentence(member, key, defect))
    })
}

/// The first datatype defect among the attribute values of one map member, as the KEY it was
/// found under and the fact about it — or `None` when every value under it carries exactly one
/// datatype.
///
/// The key is the member's OWN entry, whichever level of that entry's value the defect sits
/// at: what a caller fixes is the entry it wrote.
fn first_defect(carried: &Map<String, Value>) -> Option<(&str, &'static str)> {
    carried
        .iter()
        .find_map(|(key, value)| Some((key.as_str(), defect_in(value)?)))
}

/// The datatype defect of ONE value, looked for in the value itself and then inside the
/// containers it nests through.
///
/// Recursive, and bounded by the JSON reader above rather than by the request: a document
/// nesting deeper than that reader descends does not parse at all, so what this walk is handed
/// is already no deeper than that reader's own limit.
///
/// A value that is not a JSON object carries no datatype the reader could count: its class is
/// what is wrong with it, which is the marshalling failure another layer answers
/// ([`super::request_layer`]), so nothing is reported for it here.
fn defect_in(value: &Value) -> Option<&'static str> {
    let stated = value.as_object()?;
    match DATATYPES
        .iter()
        .filter(|datatype| stated.contains_key(**datatype))
        .count()
    {
        0 => return Some(EMPTY_ATTRIBUTE_VALUE_MESSAGE),
        1 => {}
        _ => return Some(AMBIGUOUS_ATTRIBUTE_VALUE_MESSAGE),
    }
    if let Some(entries) = stated.get(NESTED_MAP).and_then(Value::as_object) {
        return entries.values().find_map(defect_in);
    }
    stated
        .get(NESTED_LIST)
        .and_then(Value::as_array)?
        .iter()
        .find_map(defect_in)
}

/// Where in a request body an `AttributeValue` stands, by the MEMBER it stands under and the
/// shape that member carries.
///
/// Read off the model: the members whose target reaches the union
/// (`smithy_query(query="shape", shape="com.amazonaws.dynamodb#AttributeValue")`) are `Item`
/// and `Key` (a map of values, on the item writes and on every batched and transactional
/// element of them), `Keys` (a list of such maps, on `BatchGetItem`), `ExclusiveStartKey` and
/// `ExpressionAttributeValues` (a map of values), `Parameters` and `SearchVector` (a list of
/// values), `AttributeValueList` (a list, on the legacy conditions) and `Value` (one value, on
/// an attribute update and on a legacy expectation). A member of one of those names carries
/// values WHEREVER it stands, so the walk below finds them by name however deeply the element
/// carrying them nests.
const VALUE_BEARING_MEMBERS: &[(&str, Carried)] = &[
    ("AttributeValueList", Carried::ListOfValues),
    ("ExclusiveStartKey", Carried::MapOfValues),
    ("ExpressionAttributeValues", Carried::MapOfValues),
    ("Item", Carried::MapOfValues),
    ("Key", Carried::MapOfValues),
    ("Keys", Carried::ListOfMaps),
    ("Parameters", Carried::ListOfValues),
    ("SearchVector", Carried::ListOfValues),
    ("Value", Carried::OneValue),
];

/// What one of those members carries.
#[derive(Debug, Clone, Copy)]
enum Carried {
    /// One `AttributeValue`.
    OneValue,
    /// A map of them, keyed by the attribute or substitution name.
    MapOfValues,
    /// A list of them.
    ListOfValues,
    /// A list of maps of them.
    ListOfMaps,
}

impl Carried {
    /// The first defect among the values `held` carries.
    fn defect(self, held: &Value) -> Option<String> {
        match self {
            Self::OneValue => value_defect(held),
            Self::MapOfValues => held.as_object()?.values().find_map(value_defect),
            Self::ListOfValues => held.as_array()?.iter().find_map(value_defect),
            Self::ListOfMaps => held.as_array()?.iter().find_map(|carried| {
                carried
                    .as_object()
                    .into_iter()
                    .flatten()
                    .find_map(|(_, value)| value_defect(value))
            }),
        }
    }
}

/// How a member above carries its values, or `None` for a member that carries none.
fn carried_by(member: &str) -> Option<Carried> {
    VALUE_BEARING_MEMBERS
        .iter()
        .find(|(name, _)| *name == member)
        .map(|(_, carried)| *carried)
}

/// The operations whose input reaches one of those members, sorted.
///
/// The gate on reading the body at all, for the same reason [`MEMBER_BEARING_OPERATIONS`]
/// gates the walk above: a request of any other operation carries no `AttributeValue`, so no
/// walk over its body could answer anything. Read off the model — every operation whose input
/// closure reaches the union — which is the data plane, the vector search and the export whose
/// filter substitutes values.
const VALUE_BEARING_OPERATIONS: [&str; 15] = [
    "BatchExecuteStatement",
    "BatchGetItem",
    "BatchWriteItem",
    "DeleteItem",
    "ExecuteStatement",
    "ExecuteTransaction",
    "ExportTableToPointInTime",
    "GetItem",
    "PutItem",
    "Query",
    "Scan",
    "SearchVectors",
    "TransactGetItems",
    "TransactWriteItems",
    "UpdateItem",
];

/// The sentence refusing `req`'s body for an `AttributeValue` that breaks the union's own
/// rules, WHEREVER it stands — or `None` when every value the body carries is well formed.
///
/// The sentence is the fact alone: the clause a member states an envelope of its own around it
/// is answered before this ([`enveloped_refusal`]), so what reaches here is every other
/// position, which AWS refuses with the fact and nothing else.
pub fn value_refusal(operation: &str, req: &RtRequest) -> Option<String> {
    if VALUE_BEARING_OPERATIONS.binary_search(&operation).is_err() {
        return None;
    }
    let body = req.body().bytes()?;
    let document = ::serde_json::from_slice::<Value>(body).ok()?;
    first_carried_defect(&document)
}

/// The first defect among every value the body carries, found by walking the body for the
/// members above: an element of a batch or a transaction carries them exactly as a top-level
/// request does, so the walk descends every structure and list on the way to them.
///
/// Recursive, and bounded by the JSON reader above rather than by the request, exactly as the
/// walk inside one value is ([`value_defect`]): a document nesting deeper than that reader
/// descends does not parse at all, so what this walk is handed is already no deeper than that
/// reader's own limit.
fn first_carried_defect(node: &Value) -> Option<String> {
    match node {
        Value::Object(members) => members.iter().find_map(|(name, held)| {
            match carried_by(name) {
                // The value's own tree holds no member of this kind: what is inside an
                // `AttributeValue` is read as one.
                Some(carried) => carried.defect(held),
                None => first_carried_defect(held),
            }
        }),
        Value::Array(elements) => elements.iter().find_map(first_carried_defect),
        _ => None,
    }
}

/// The set datatypes a member of which can be stated `null`, each with the sentence AWS
/// refuses that with: a `null` standing where a member of the set stands is no member of it at
/// all, and each set type words it its own way.
const NULL_SET_MEMBER_SENTENCES: [(&str, &str); 3] = [
    ("SS", "An string set may not have a null string as a member"),
    ("NS", "Numeric sets may not contain null"),
    ("BS", "Binary sets may not contain null or empty values"),
];

/// The defect of ONE value and of the values it nests, as the whole sentence for it.
///
/// Two rules, over the same value: the union's own arity ([`defect_in`]), and a `null` listed
/// as a member of a set. Both are stated in the class a refusal about a value the request
/// CARRIED is stated in — the arity fact alone, and the set rule behind the parameter-value
/// clause this service states such a rule in
/// ([`crate::state::INVALID_PARAMETER_CLAUSE`]).
///
/// Recursive, and bounded by the JSON reader rather than by the request, for the reason
/// [`defect_in`] states.
fn value_defect(value: &Value) -> Option<String> {
    let stated = value.as_object()?;
    match DATATYPES
        .iter()
        .filter(|datatype| stated.contains_key(**datatype))
        .count()
    {
        0 => return Some(EMPTY_ATTRIBUTE_VALUE_MESSAGE.to_string()),
        1 => {}
        _ => return Some(AMBIGUOUS_ATTRIBUTE_VALUE_MESSAGE.to_string()),
    }
    let null_member = NULL_SET_MEMBER_SENTENCES.iter().find(|(datatype, _)| {
        stated
            .get(*datatype)
            .and_then(Value::as_array)
            .is_some_and(|members| members.iter().any(Value::is_null))
    });
    if let Some((_, sentence)) = null_member {
        return Some(format!(
            "{}: {sentence}",
            crate::state::INVALID_PARAMETER_CLAUSE
        ));
    }
    if let Some(entries) = stated.get(NESTED_MAP).and_then(Value::as_object) {
        return entries.values().find_map(value_defect);
    }
    stated
        .get(NESTED_LIST)
        .and_then(Value::as_array)?
        .iter()
        .find_map(value_defect)
}
