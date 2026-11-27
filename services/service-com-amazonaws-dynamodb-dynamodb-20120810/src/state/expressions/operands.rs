// SPDX-License-Identifier: CC0-1.0

//! What an expression's OPERANDS may be: the closed domains the functions and the
//! grammar hold a literal operand to.
//!
//! These are facts of the TEXT one request carries, so they are settled where the
//! expression is read rather than where an item is compared: an operand outside its
//! domain is the caller's mistake whatever the addressed table holds, and a request
//! carrying one is refused before anything is read. [`super`] re-exports them, so every
//! caller still names them `crate::state::…`.

use super::*;

/// Whether a bare word begins with a digit, which is what tells a numeric literal from
/// an attribute name written out in an expression ([`ExprParser::path_name`]).
pub(super) fn starts_with_digit(word: &str) -> bool {
    word.starts_with(|character: char| character.is_ascii_digit())
}

/// What ONE operand position of a function takes.
///
/// Every function of the expression languages declares each of its operands as one or the
/// other, and the declaration is read off the documented signature rather than guessed
/// from the operand: `attribute_exists (path)`, `begins_with (path, substr)`,
/// `contains (path, operand)`, `attribute_type (path, type)`, `size (path)`,
/// `if_not_exists (path, operand)` and `list_append (operand, operand)` (the
/// `ConditionExpression` and `UpdateExpression` function references).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum OperandKind {
    /// A document path: the location in the item the function reads.
    Path,
    /// A value the request carried: a `:placeholder` standing for one.
    Value,
    /// Either — a value, or a location the value is read from.
    Either,
}

/// The documented signature of one function the expression languages define, or `None`
/// for an identifier that names no function of theirs.
///
/// The ONE place the arity and the operand kinds of these functions are stated, so the
/// refusals a malformed call earns — the wrong NUMBER of operands
/// ([`incorrect_operand_count`]), a value where a path belongs
/// ([`requires_document_path`]) and a path where a value belongs
/// ([`incorrect_operand_type`]) — are decided by the signature and not by where a
/// recursive-descent parser happened to stop.
///
/// A position the references call an OPERAND takes either: a value the request carried, or
/// a path the value is read from. "contains (path, operand) ... The path and the operand
/// must be distinct; for example, contains (a, a) returns an error" and
/// "begins_with (path, substr)" read the second operand off the item exactly as the first
/// is read, so what it holds is known only once an item is read. The one position that
/// needs a literal is `attribute_type (path, type)`'s type code, which names a member of a
/// closed set ([`check_attribute_type_name`]).
///
/// `contains` is the function whose SEARCHED operand takes either as well: the collection it
/// searches may be one the REQUEST carried rather than one the item holds, which is how a
/// membership test over a list of expression-attribute values is written
/// (`contains(:candidates, #attribute)`), and AWS is observed to evaluate that call per item
/// instead of refusing it. Both positions are then read the same way, and what each holds is
/// known only once an item is read.
///
/// `begins_with` is read the same way, and for the same reason: AWS is observed to evaluate
/// `begins_with(:text, #attribute)` per item — the tested text may be one the REQUEST
/// carried and the prefix one the item holds — and to answer the items that satisfy it
/// rather than to refuse the call. What an operand in either of those positions may BE is
/// still the function's own domain ([`check_begins_with_operand`]). Only `if_not_exists`,
/// whose first operand is the location
/// an update writes when it is absent, needs a path in that position, and the
/// `attribute_exists`/`attribute_not_exists`/`size` and `attribute_type` positions above
/// each state a location or a type NAME rather than a value to compare.
pub(super) fn function_signature(name: &str) -> Option<&'static [OperandKind]> {
    const PATH: &[OperandKind] = &[OperandKind::Path];
    const PATH_VALUE: &[OperandKind] = &[OperandKind::Path, OperandKind::Value];
    const PATH_EITHER: &[OperandKind] = &[OperandKind::Path, OperandKind::Either];
    const EITHER_EITHER: &[OperandKind] = &[OperandKind::Either, OperandKind::Either];
    match name {
        "attribute_exists" | "attribute_not_exists" | "size" => Some(PATH),
        "attribute_type" => Some(PATH_VALUE),
        "if_not_exists" => Some(PATH_EITHER),
        "begins_with" | "contains" | "list_append" => Some(EITHER_EITHER),
        _ => None,
    }
}

/// The functions a CONDITION may call, with their signatures: a closed set — "These
/// function names are case-sensitive" and the `ConditionExpression` doc lists exactly
/// these — so an identifier called like a function that is not one of them names no
/// function of this grammar ([`unknown_function`]).
pub(super) fn condition_function_signature(name: &str) -> Option<&'static [OperandKind]> {
    match name {
        "attribute_exists" | "attribute_not_exists" | "attribute_type" | "begins_with"
        | "contains" => function_signature(name),
        _ => None,
    }
}

/// The function that reports the LENGTH of what a path addresses, which is an operand of
/// a comparison rather than a condition of its own.
pub(super) const SIZE_FUNCTION: &str = "size";

/// The range comparator as an expression writes it, which is what its refusals name.
pub(super) const BETWEEN_OPERATOR: &str = "BETWEEN";

/// How a refusal names an operand that reads a LOCATION where the signature declares a
/// value: the kind of operand it is, which is all the request states about it — what the
/// location holds is known only once an item is read.
const PATH_OPERAND_TYPE: &str = "PATH";

/// Hold one operand to the kind its position declares, where the REQUEST settles it.
///
/// A `:placeholder` written where a path belongs names no location at all, and a path
/// written where a value belongs carries none: both are facts of the expression's text, so
/// both are refused as the expression is read rather than evaluated against an item.
pub(super) fn check_operand_kind(
    member: &str,
    function: &str,
    kind: OperandKind,
    operand: &Operand,
) -> Result<(), RequestError> {
    match (kind, operand) {
        (OperandKind::Value, Operand::Path(_) | Operand::Size(_)) => Err(incorrect_operand_type(
            member,
            function,
            PATH_OPERAND_TYPE,
        )),
        (OperandKind::Path, Operand::Value(_)) => Err(requires_document_path(member, function)),
        _ => Ok(()),
    }
}

/// Whether ONE operator or function breaks the rule that its first operand is distinct
/// from the remaining ones — the first operand, when it does — the rule the reference
/// states of `contains`: "contains (path, operand) ... The path and the operand must be
/// distinct; for example, contains (a, a) returns an error" (the `ConditionExpression`
/// function reference).
///
/// An operand that reads the very location the first operand reads asks whether a value
/// relates to ITSELF, which is settled by the expression's own text rather than by any
/// item: `=` holds for every item carrying the attribute, `<` for none, and `contains` for
/// every item whose collection holds its own value. AWS states that as one rule over every
/// OPERATOR AND FUNCTION that takes a first operand and remaining ones — the sentence
/// names "this operator or function" — so each of them is read for it where its operands
/// are read.
///
/// The rule is about two operands naming ONE LOCATION, so it is asked only where both of
/// them name one: a literal the request carried has no path to be distinct from, and the
/// values it compares are the request's own rather than one attribute read twice.
///
/// The defect is REPORTED once the whole expression has parsed
/// ([`ExprParser::check_distinct_first_operands`]), beside the other rules about an
/// expression AWS states only of text its grammar accepts.
pub(super) fn non_distinct_first_operand<'o>(
    first: &'o Operand,
    rest: &[&Operand],
) -> Option<&'o DocumentPath> {
    let Operand::Path(path) = first else {
        return None;
    };
    rest.iter()
        .any(|operand| matches!(operand, Operand::Path(other) if other == path))
        .then_some(path)
}

/// How that defect is refused: the operator or function as the EXPRESSION writes it, and
/// the first operand rendered as the token list AWS's refusals print a document path as
/// ([`DocumentPath::render_tokens`]).
pub(super) fn non_distinct_operands(
    member: &str,
    operator: &str,
    first_operand: &str,
) -> RequestError {
    invalid_expression_member(
        member,
        &format!(
            "The first operand must be distinct from the remaining operands for this operator \
             or function; operator: {operator}, first operand: {first_operand}"
        ),
    )
}

/// Hold one operand of an ORDERING comparison to the types DynamoDB orders, where the
/// request stated that operand as a literal.
///
/// "For type Number, value comparisons are numeric. String value comparisons ... are
/// based on ASCII character code values. For Binary, DynamoDB treats each byte ... as
/// unsigned when it compares binary values" (the `AttributeValueList` doc) — and nothing
/// for the other types, so a value of any of them cannot satisfy `<`, `<=`, `>`, `>=` or
/// `BETWEEN` whatever the addressed item holds. That makes it the caller's mistake and not
/// an unmet condition, refused when the expression is read; an operand that READS an
/// attribute is left to the evaluation, which is the only place its type is known.
pub(super) fn check_orderable_operand(
    member: &str,
    operator: &str,
    operand: &Operand,
) -> Result<(), RequestError> {
    let Operand::Value(value) = operand else {
        return Ok(());
    };
    if matches!(
        value,
        AttributeValue::S(_) | AttributeValue::N(_) | AttributeValue::B(_)
    ) {
        return Ok(());
    }
    Err(incorrect_operand_type(
        member,
        operator,
        attribute_value_type(value),
    ))
}

/// The two bounds of a `BETWEEN` the request stated as LITERALS, held to the order the
/// comparator requires ([`check_between_bounds`]).
pub(super) fn check_between_operands(
    member: &str,
    low: &Operand,
    high: &Operand,
) -> Result<(), RequestError> {
    let (Operand::Value(low), Operand::Value(high)) = (low, high) else {
        return Ok(());
    };
    check_between_bounds(member, low, high)
}

/// The `AttributeValue` type an operand IS, where the REQUEST alone settles it, and
/// `None` for one whose type only the stored item knows.
///
/// Two operand forms carry their type in the expression: a value the request bound to a
/// `:placeholder` is of the type it was sent as, and `size (path)` — "Returns a number
/// representing an attribute's size" (the `ConditionExpression` function reference) — is a
/// NUMBER whatever the location it measures holds. So the domain of an operator or
/// function can be held against either of them where the expression is READ, which is
/// where every other fact of the request's own text is settled. An operand that reads an
/// attribute carries no type here: what that location holds is known only once an item is
/// read.
pub(super) fn stated_operand_type(operand: &Operand) -> Option<&'static str> {
    match operand {
        Operand::Value(value) => Some(attribute_value_type(value)),
        Operand::Size(_) => Some(SIZE_OPERAND_TYPE),
        Operand::Path(_) => None,
    }
}

/// The type `size (path)` reports, which is the type a refusal names for an operand
/// written as that call: "Returns a number representing an attribute's size".
const SIZE_OPERAND_TYPE: &str = "N";

/// `begins_with` relates one TEXT to another, so each of its operands is a string or a
/// binary value, and an operand of any other type — in EITHER position — is refused when
/// the expression is read.
///
/// "begins_with (path, substr) — true if the attribute specified by path begins with a
/// particular substring" (the `ConditionExpression` doc): only `S` and `B` have prefixes
/// to compare and only they can be compared against one, so a number or a collection can
/// be neither the tested text nor the prefix. Both positions take an operand
/// ([`function_signature`]), and an operand whose type the REQUEST settles — a value it
/// bound, or the number a `size` call reports ([`stated_operand_type`]) — is held to that
/// domain whether or not the addressed table holds an item to evaluate the call against.
/// An operand that reads an attribute is left to the evaluation, which is the only place
/// its type is known.
pub(super) fn check_begins_with_operand(
    member: &str,
    operand: &Operand,
) -> Result<(), RequestError> {
    let Some(datatype) = stated_operand_type(operand) else {
        return Ok(());
    };
    if matches!(datatype, "S" | "B") {
        return Ok(());
    }
    Err(incorrect_operand_type(member, "begins_with", datatype))
}

/// The type codes a type test may name, in the order AWS's own refusal lists them.
///
/// Source: this service's Smithy model, the members of the `AttributeValue` union
/// (`smithy_query(query="shape",shape="AttributeValue")`) — the codes a stored value can
/// carry are exactly the codes a test of its type can name.
pub const ATTRIBUTE_TYPE_NAMES: [&str; 10] =
    ["S", "SS", "N", "NS", "B", "BS", "BOOL", "NULL", "L", "M"];

/// `attribute_type`'s second operand names a TYPE, so it is held to that closed set as
/// the expression is READ rather than as an item is compared.
///
/// The operand is a literal the request already carried, and a type test naming no type
/// can select nothing, so the request is refused whether or not the addressed table
/// holds an item to evaluate it against — a token that is no type code is the caller's
/// mistake, not an empty result. A code the set does not hold is named back as the
/// request spelled it, beside the whole set; a literal that is not a string at all is
/// the operand-type refusal, since the function takes a type NAME. An operand that
/// reads an attribute instead of naming a literal is left to the evaluation, which is
/// the only place its type is known.
pub(super) fn check_attribute_type_name(
    member: &str,
    operand: &Operand,
) -> Result<(), RequestError> {
    let Operand::Value(value) = operand else {
        return Ok(());
    };
    let AttributeValue::S(name) = value else {
        return Err(incorrect_operand_type(
            member,
            "attribute_type",
            attribute_value_type(value),
        ));
    };
    if ATTRIBUTE_TYPE_NAMES.contains(&name.as_str()) {
        return Ok(());
    }
    Err(invalid_expression_member(
        member,
        &format!(
            "Invalid attribute type name found; type: {name}, valid types: {{{}}}",
            ATTRIBUTE_TYPE_NAMES.join(",")
        ),
    ))
}
