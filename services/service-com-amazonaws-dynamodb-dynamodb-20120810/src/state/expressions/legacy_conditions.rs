// SPDX-License-Identifier: CC0-1.0

use super::*;

/// Hold the `AttributeValueList` of ONE legacy condition entry to the rules that bind the
/// list AS A WHOLE: its members share one `AttributeValue` type
/// ([`MIXED_OPERAND_TYPES_CLAUSE`]), each is of a type the comparison compares
/// ([`check_legacy_operand_types`]), and a `BETWEEN` states its two bounds in the order the
/// comparator reads them ([`REVERSED_BETWEEN_CLAUSE`]).
///
/// No rule here depends on a stored item, so a legacy condition breaking one is refused as
/// the invalid parameter value it is BEFORE anything is read, rather than evaluated into a
/// condition that can hold for nothing. All three hold for every member a legacy condition
/// arrives in — `Expected`, the legacy filters, `KeyConditions` — and `member` decides only
/// whether the sentence is stated inside the validation envelope
/// ([`envelopes_legacy_refusals`]).
///
/// An entry naming NO comparison carries no operands of one, which is a defect of the
/// entry itself ([`attribute_value_list_without_comparator`]) rather than of its list, so
/// every caller here has a comparison to hold the list to.
pub fn check_legacy_operand_list(
    member: &str,
    operator: &str,
    operands: &[AttributeValue],
) -> Result<(), RequestError> {
    let enveloped = envelopes_legacy_refusals(member);
    let refusal = |clause: &str| {
        expression_refusal(enveloped, format!("{INVALID_PARAMETER_CLAUSE}: {clause}"))
    };
    if let Some((first, rest)) = operands.split_first() {
        let one_type = attribute_value_type(first);
        if rest
            .iter()
            .any(|operand| attribute_value_type(operand) != one_type)
        {
            return Err(refusal(MIXED_OPERAND_TYPES_CLAUSE));
        }
    }
    // The operand types this comparison compares at all, which is a rule of the operator
    // over each member of the list.
    if let Err(clause) = check_legacy_operand_types(operator, operands) {
        return Err(expression_refusal(enveloped, clause));
    }
    if operator == BETWEEN_OPERATOR {
        if let [low, high] = operands {
            if compare_values(low, high) == Some(Ordering::Greater) {
                return Err(refusal(REVERSED_BETWEEN_CLAUSE));
            }
        }
    }
    Ok(())
}

/// How an `Expected` entry carrying an `AttributeValueList` and no `ComparisonOperator` is
/// refused.
///
/// "Use AttributeValueList to specify one or more values to compare against an attribute.
/// Use ComparisonOperator to specify how you want to perform the comparison" (the
/// `ExpectedAttributeValue` doc): the list is the operands of a comparison, so an entry
/// stating one without naming a comparison states values nothing is compared against. AWS
/// names the attribute whose entry carried it, like every other refusal about one entry of
/// this map, and states it where every other refusal about this map is stated
/// ([`envelopes_legacy_refusals`]).
pub(super) fn attribute_value_list_without_comparator(name: &str) -> RequestError {
    expression_refusal(
        envelopes_legacy_refusals(EXPECTED_MEMBER),
        format!(
            "{INVALID_PARAMETER_CLAUSE}: AttributeValueList can only be used with a \
             ComparisonOperator for Attribute: {name}"
        ),
    )
}

/// Hold every VALUE one legacy condition map compares against to the documented value
/// rules, exactly as a value a write stores or an expression binds is held to them.
///
/// The rules belong to the value's TYPE and to this service's own ceilings on the work
/// one value can ask for ([`check_attribute_value`] — the documented nesting depth, the
/// item ceiling, the members a collection may carry, the numeric domain and the set
/// rules), so they hold wherever a request carries a value: an `Expected` entry's
/// `Value` or `AttributeValueList`, and a `ScanFilter`/`QueryFilter` entry's
/// `AttributeValueList`, are values a request carries like any other.
///
/// `member` is the map they arrived in, which decides only whether the clause is stated
/// inside the validation envelope ([`envelopes_legacy_refusals`]).
pub fn check_legacy_condition_values<'v>(
    member: &str,
    values: impl Iterator<Item = (&'v str, &'v AttributeValue)>,
) -> Result<(), RequestError> {
    let enveloped = envelopes_legacy_refusals(member);
    for (name, value) in values {
        check_attribute_value(name, value).map_err(|error| {
            if enveloped {
                error
            } else {
                without_envelope(error)
            }
        })?;
    }
    Ok(())
}

/// Hold every value the legacy `Expected` map carries — each entry's `Value` and every
/// element of its `AttributeValueList` — to the documented value rules.
///
/// `Expected` is the THIRD place one write request can carry an attribute value, beside
/// the `Item` it stores and the `ExpressionAttributeValues` its expression binds, and
/// every one of them bounds the work the request can ask for, so the three are held to
/// one bound ([`check_attribute_value`]) before anything walks them. Stated here as the
/// one reader of that map's values, so a handler bounding its own request's values and
/// [`check_write_condition`] cannot read them two different ways.
pub fn check_expected_condition_values(
    expected: Option<&HashMap<String, ExpectedAttributeValue>>,
) -> Result<(), RequestError> {
    let Some(expected) = expected else {
        return Ok(());
    };
    check_legacy_condition_values(
        EXPECTED_MEMBER,
        expected.iter().flat_map(|(name, condition)| {
            condition
                .value
                .iter()
                .chain(condition.attribute_value_list.iter().flatten())
                .map(move |value| (name.as_str(), value))
        }),
    )
}

/// Whether the condition a conditional write states is satisfied by the stored item.
///
/// One entry point for all three single-item writes: each states its condition the
/// same two ways — the `ConditionExpression` language, or the legacy `Expected` map
/// combined by `ConditionalOperator` — and they are incompatible with each other
/// ("Use ConditionExpression instead"; a request may not use both).
pub fn write_condition_satisfied(
    condition_expression: Option<&str>,
    expected: Option<&HashMap<String, ExpectedAttributeValue>>,
    conditional_operator: Option<&str>,
    substitutions: Substitutions<'_>,
    item: Option<&Item>,
) -> Result<bool, RequestError> {
    // Every fact of the REQUEST its condition carries is settled first, by the same
    // check the handlers make before they look the table up, so one condition cannot be
    // judged by two different readings of its own members.
    check_write_condition(
        condition_expression,
        expected,
        conditional_operator,
        substitutions,
    )?;
    let expected = expected.filter(|map| !map.is_empty());
    match (condition_expression, expected) {
        (Some(expression), None) => {
            ParsedCondition::parse("ConditionExpression", expression, substitutions)?.evaluate(item)
        }
        (None, Some(expected)) => expected_satisfied(expected, conditional_operator, item),
        // A request stating its condition both ways was refused above, and one stating
        // it neither way asks for nothing to be checked.
        (Some(_), Some(_)) | (None, None) => Ok(true),
    }
}

/// Settle every fact of a conditional write's CONDITION that the request alone
/// decides: which members it stated the condition through, whether the expression
/// parses, whether each `Expected` entry states something checkable, and whether the
/// values it compares against are values this service accepts.
///
/// None of it depends on the table or on the item, so the three single-item writes
/// call it BEFORE they look their table up: a request that cannot be served however
/// the store looks is refused as the malformed request it is, rather than reported as
/// a missing table. [`write_condition_satisfied`] runs the same check, so the two
/// readings of the same members cannot drift apart.
pub fn check_write_condition(
    condition_expression: Option<&str>,
    expected: Option<&HashMap<String, ExpectedAttributeValue>>,
    conditional_operator: Option<&str>,
    substitutions: Substitutions<'_>,
) -> Result<(), RequestError> {
    let expected = expected.filter(|map| !map.is_empty());
    match (condition_expression, expected) {
        (Some(_), Some(_)) => Err(expression_conflict("Expected", "ConditionExpression")),
        (Some(expression), None) => {
            if conditional_operator.is_some() {
                return Err(expression_conflict(
                    "ConditionalOperator",
                    "ConditionExpression",
                ));
            }
            ParsedCondition::parse("ConditionExpression", expression, substitutions)?;
            Ok(())
        }
        (None, Some(expected)) => {
            // The member combines the entries of `Expected`, so the map it was sent with
            // has to hold two or more of them.
            check_conditional_operator_companion(
                EXPECTED_MEMBER,
                conditional_operator,
                expected.len(),
            )?;
            // Every value the map compares against is held to the documented value rules,
            // before the stored item is read against any of them.
            check_expected_condition_values(Some(expected))?;
            conditional_operator_any(conditional_operator)?;
            for (name, condition) in expected {
                // Read against NO item: every refusal an entry earns is a fact of the
                // entry, so the walk that evaluates one surfaces exactly those.
                expected_entry_satisfied(name, condition, None)?;
            }
            Ok(())
        }
        (None, None) => check_conditional_operator_companion(EXPECTED_MEMBER, conditional_operator, 0),
    }
}

/// How the entries of a legacy condition map are combined: "AND - If all of the
/// conditions evaluate to true ... OR - If at least one of the conditions evaluate to
/// true" (`ConditionalOperator`), with `AND` the default — answering whether ANY entry
/// holding is enough.
///
/// One reading for every legacy surface that carries the member (`Expected`,
/// `ScanFilter`), so a value outside the model's enum is refused in one wording.
pub fn conditional_operator_any(conditional_operator: Option<&str>) -> Result<bool, RequestError> {
    match conditional_operator {
        None | Some("AND") => Ok(false),
        Some("OR") => Ok(true),
        Some(other) => Err(RequestError::Validation(format!(
            "ConditionalOperator must be AND or OR, got {other}"
        ))),
    }
}

/// Whether the legacy `Expected` map holds for the stored item.
pub(super) fn expected_satisfied(
    expected: &HashMap<String, ExpectedAttributeValue>,
    conditional_operator: Option<&str>,
    item: Option<&Item>,
) -> Result<bool, RequestError> {
    let any = conditional_operator_any(conditional_operator)?;
    let absent = Item::new();
    let item = item.unwrap_or(&absent);
    let mut satisfied = !any;
    for (name, condition) in expected {
        let held = item.get(name);
        let holds = expected_entry_satisfied(name, condition, held)?;
        if any {
            satisfied |= holds;
        } else {
            satisfied &= holds;
        }
    }
    Ok(satisfied)
}

/// One entry of the legacy `Expected` map.
///
/// "Value and Exists are incompatible with AttributeValueList and ComparisonOperator.
/// Note that if you use both sets of parameters at once, DynamoDB will return a
/// ValidationException"; with neither set, there is nothing to check.
///
/// Every refusal about the entry's own members is stated where every other refusal about
/// this map is stated ([`envelopes_legacy_refusals`]); the sentences themselves are the same
/// on every write.
pub(super) fn expected_entry_satisfied(
    name: &str,
    condition: &ExpectedAttributeValue,
    held: Option<&AttributeValue>,
) -> Result<bool, RequestError> {
    let shape_refusal =
        |clause: String| expression_refusal(envelopes_legacy_refusals(EXPECTED_MEMBER), clause);
    let comparison = condition.comparison_operator.as_deref();
    let operands = condition
        .attribute_value_list
        .as_deref()
        .filter(|values| !values.is_empty());
    // "Value and Exists are incompatible with AttributeValueList and ComparisonOperator":
    // the pairs that cannot be used together are the ones stating the SAME half of the
    // condition twice — the test itself (`Exists` beside `ComparisonOperator`) and the
    // value it is made against (`Value` beside `AttributeValueList`) — and AWS names the
    // pair the entry carried. An entry naming a comparison and ONE value through `Value`
    // states each half once, so it is a condition this service EVALUATES rather than a
    // conflict.
    let conflict = if comparison.is_some() && condition.exists.is_some() {
        Some(("Exists", "ComparisonOperator"))
    } else if operands.is_some() && condition.value.is_some() {
        Some(("Value", "AttributeValueList"))
    } else {
        None
    };
    if let Some((legacy, newer)) = conflict {
        return Err(shape_refusal(format!(
            "One or more parameter values were invalid: {legacy} and {newer} cannot be used \
             together for Attribute: {name}"
        )));
    }
    // An entry carrying the operands of a comparison and naming no comparison to make with
    // them states nothing this service can check
    // ([`attribute_value_list_without_comparator`]).
    if comparison.is_none() && operands.is_some() {
        return Err(attribute_value_list_without_comparator(name));
    }
    if let Some(operator) = comparison {
        // An operator that compares a value needs one to compare, and an `Expected`
        // entry can carry it in either of two members: AWS names both as what the
        // entry has to state beside the operator it named. `Value` carries the ONE
        // operand the comparison is made against where the entry states no list.
        let single = condition.value.clone().map(|value| vec![value]);
        let operands: &[AttributeValue] = operands.or(single.as_deref()).unwrap_or(&[]);
        if operands.is_empty() && takes_operands(operator) {
            return Err(shape_refusal(format!(
                "{INVALID_PARAMETER_CLAUSE}: Value or AttributeValueList must be used with \
                 ComparisonOperator: {operator} for Attribute: {name}"
            )));
        }
        // The operands are held to what the OPERATOR takes before the list is held to
        // what the list states as a whole: how many operands the comparison takes is the
        // operator's rule ([`check_legacy_comparison`]) and their types and order are the
        // list's ([`check_legacy_operand_list`], which states them behind the member this
        // entry arrived in), and a request breaking both earns the count. The comparison
        // below settles the count again for every item it reads, which is where a reader
        // that never ran this pass gets it.
        check_legacy_comparison(operator, operands)?;
        check_legacy_operand_list(EXPECTED_MEMBER, operator, operands)?;
        return legacy_comparison_matches(operator, held, operands);
    }
    match (condition.value.as_ref(), condition.exists) {
        // "If you supply a Value all by itself, DynamoDB assumes the attribute exists"
        // — the attribute must be there AND match.
        (Some(value), None | Some(true)) => {
            Ok(held.is_some_and(|held| attribute_values_equal(held, value)))
        }
        // "Exists is false but you also provide a Value" is refused, as is an `Exists`
        // of true with nothing to check.
        (Some(_), Some(false)) => Err(shape_refusal(format!(
            "{INVALID_PARAMETER_CLAUSE}: Value cannot be used when Exists is false for \
             Attribute: {name}"
        ))),
        (None, Some(true)) => Err(shape_refusal(format!(
            "{INVALID_PARAMETER_CLAUSE}: Value must be provided when Exists is true for \
             Attribute: {name}"
        ))),
        (None, Some(false)) => Ok(held.is_none()),
        // An entry that states NEITHER member expects nothing: `Exists` defaults to
        // true — "If you supply a Value all by itself, DynamoDB assumes the attribute
        // exists" — and an expectation that the attribute exists needs the value it is
        // expected to hold. AWS names `Exists` as the member that was left out.
        (None, None) => Err(shape_refusal(format!(
            "{INVALID_PARAMETER_CLAUSE}: Value must be provided when Exists is null for \
             Attribute: {name}"
        ))),
    }
}

/// Whether one legacy `ComparisonOperator` holds between the stored attribute and the
/// request's `AttributeValueList`, as the `ComparisonOperator` doc defines each one.
pub fn legacy_comparison_matches(
    operator: &str,
    held: Option<&AttributeValue>,
    operands: &[AttributeValue],
) -> Result<bool, RequestError> {
    check_legacy_comparison(operator, operands)?;
    Ok(match operator {
        // "NOT_NULL : the attribute exists"; "NULL : the attribute does not exist".
        "NOT_NULL" => held.is_some(),
        "NULL" => held.is_none(),
        _ => {
            let Some(held) = held else {
                // Every other operator compares a value, and the item holds none here.
                // A comparison needs something to compare, so an item that does not
                // carry the attribute at all satisfies none of them — the one documented
                // exception is `NE`: "NE : Not equal ... If the item contains an
                // AttributeValue of a different type than the one provided in the
                // request, the value does not match" (`ComparisonOperator`), and an item
                // carrying no value for the name is not equal to the one compared
                // against. Every other operator — the negated `NOT_CONTAINS` included —
                // selects only the items that HAVE the attribute, which is what makes a
                // legacy filter answer the item set `attribute_exists` plus the
                // expression form of the comparator answers.
                return Ok(operator == "NE");
            };
            match operator {
                "EQ" => attribute_values_equal(held, &operands[0]),
                "NE" => !attribute_values_equal(held, &operands[0]),
                "LT" => compare_values(held, &operands[0]) == Some(Ordering::Less),
                "LE" => matches!(
                    compare_values(held, &operands[0]),
                    Some(Ordering::Less | Ordering::Equal)
                ),
                "GT" => compare_values(held, &operands[0]) == Some(Ordering::Greater),
                "GE" => matches!(
                    compare_values(held, &operands[0]),
                    Some(Ordering::Greater | Ordering::Equal)
                ),
                "CONTAINS" => contains_value(held, &operands[0]),
                "NOT_CONTAINS" => !contains_value(held, &operands[0]),
                "BEGINS_WITH" => begins_with_value(held, &operands[0]),
                "IN" => operands
                    .iter()
                    .any(|candidate| attribute_values_equal(held, candidate)),
                _ => matches!(
                    compare_values(held, &operands[0]),
                    Some(Ordering::Greater | Ordering::Equal)
                ) && matches!(
                    compare_values(held, &operands[1]),
                    Some(Ordering::Less | Ordering::Equal)
                ),
            }
        }
    })
}

/// How many operands one legacy comparison takes, as the `ComparisonOperator` doc
/// defines each one: none for the two existence tests, two for `BETWEEN`, as many as
/// the request chooses for `IN` ([`usize::MAX`] standing for that), and one for every
/// other comparison. `None` for an operator this service does not serve at all.
pub(super) fn required_operands(operator: &str) -> Option<usize> {
    Some(match operator {
        "NULL" | "NOT_NULL" => 0,
        "BETWEEN" => 2,
        "IN" => usize::MAX,
        "EQ" | "NE" | "LE" | "LT" | "GE" | "GT" | "CONTAINS" | "NOT_CONTAINS" | "BEGINS_WITH" => 1,
        _ => return None,
    })
}

/// Whether a comparison this service serves compares a VALUE, so an entry naming it
/// has to state one — which is what tells an `Expected` entry that named an operator
/// and no value apart from the two existence tests, which state the whole condition in
/// the operator alone.
pub(super) fn takes_operands(operator: &str) -> bool {
    required_operands(operator).is_some_and(|required| required > 0)
}

/// Whether one legacy comparison is one the service serves at all, and whether it was
/// given the operands its operator takes — the facts of the REQUEST, so a reader that
/// evaluates a legacy condition map against many items settles them once, before it
/// reads anything.
///
/// The operand count is part of each operator's definition in the `ComparisonOperator`
/// doc, so a request that gives the wrong number of them is refused rather than guessed
/// at.
pub fn check_legacy_comparison(
    operator: &str,
    operands: &[AttributeValue],
) -> Result<(), RequestError> {
    let Some(required) = required_operands(operator) else {
        return Err(RequestError::Validation(format!(
            "ComparisonOperator must be one of EQ, NE, IN, LE, LT, GE, GT, BETWEEN, NOT_NULL, \
             NULL, CONTAINS, NOT_CONTAINS or BEGINS_WITH, got {operator}"
        )));
    };
    let count_ok = if required == usize::MAX {
        !operands.is_empty()
    } else {
        operands.len() == required
    };
    if !count_ok {
        return Err(RequestError::Validation(invalid_argument_count(operator)));
    }
    // The one operator whose operand count the request chooses. The documented 100-value
    // ceiling belongs to the EXPRESSION spelling of this comparison ([`MAX_IN_OPERANDS`]);
    // this list carries no documented count at all, so what bounds it is that it IS a
    // collection the request carried, held to the ceiling every collection a request carries
    // is held to ([`MAX_COLLECTION_MEMBERS`]) — checked here, before the list is compared
    // against anything, so the work one entry costs per item read is a function of that
    // constant rather than of the request body.
    if required == usize::MAX && operands.len() > MAX_COLLECTION_MEMBERS {
        return Err(RequestError::Validation(invalid_argument_count(operator)));
    }
    // Whether each operand is of a type the operator compares is a rule of the LIST this
    // entry carried, stated behind the member that carried it
    // ([`check_legacy_operand_list`]).
    Ok(())
}

/// Hold a legacy comparison's operands to the TYPES that comparison can compare, as the
/// `ComparisonOperator` doc states them per operator:
///
/// * "BEGINS_WITH : Checks for a prefix. ... AttributeValueList can contain only one
///   AttributeValue of type String or Binary (not a Number or a set type)" — only those two
///   types have a prefix;
/// * the ordering comparisons, the two subsequence tests and the two multi-value
///   comparisons take "only one AttributeValue element of type String, Number, or Binary
///   (not a set type)" (`LE`, `LT`, `GE`, `GT`, `CONTAINS`, `NOT_CONTAINS`), "one or more
///   AttributeValue elements of type String, Number, or Binary" (`IN`) and "two
///   AttributeValue elements of the same type, either String, Number, or Binary (not a set
///   type)" (`BETWEEN`);
/// * `EQ` and `NE` are "supported for all data types, including lists and maps", so they
///   compare whatever a request carries;
/// * the two existence tests compare no value at all (their operand count is 0).
///
/// The two SUBSEQUENCE tests are the one pair whose admitted set is WIDER than the sentence
/// above: what they look for is a single value inside the value the item holds — "checks for a
/// subsequence, or value in a set" (`ComparisonOperator`, the `CONTAINS` entry) — so every
/// SCALAR operand is something that can be looked for, and one that cannot be found simply
/// matches nothing rather than making the read a defect. Recorded behaviour bears that out: a
/// boolean or null operand is served and selects no item, while an operand that is itself a
/// COLLECTION — a document type, and the set types the operator's own doc excludes — names no
/// single value to look for and is refused.
///
/// An operand outside its comparison's set can match nothing whatever the addressed item
/// holds, so it is the caller's mistake and refused before anything is read. `None` for an
/// operator this service does not serve, which [`check_legacy_comparison`] answers for.
pub(super) fn legacy_operand_types(operator: &str) -> Option<&'static [&'static str]> {
    const TEXT_NUMBER_BINARY: &[&str] = &["S", "N", "B"];
    const TEXT_BINARY: &[&str] = &["S", "B"];
    /// Every type whose value is ONE scalar, which is what a subsequence test looks for.
    const SCALARS: &[&str] = &["S", "N", "B", "BOOL", "NULL"];
    match operator {
        "EQ" | "NE" | "NULL" | "NOT_NULL" => None,
        "BEGINS_WITH" => Some(TEXT_BINARY),
        "CONTAINS" | "NOT_CONTAINS" => Some(SCALARS),
        _ => Some(TEXT_NUMBER_BINARY),
    }
}

/// Hold a legacy comparison's operands to the types [`legacy_operand_types`] states for it.
///
/// AWS names the comparison and the `AttributeValue` type of the operand it does not take,
/// both read off the request.
pub(super) fn check_legacy_operand_types(
    operator: &str,
    operands: &[AttributeValue],
) -> Result<(), String> {
    let Some(allowed) = legacy_operand_types(operator) else {
        return Ok(());
    };
    for operand in operands {
        let datatype = attribute_value_type(operand);
        if !allowed.contains(&datatype) {
            return Err(format!(
                "{INVALID_PARAMETER_CLAUSE}: ComparisonOperator {operator} is not valid for \
                 {datatype} AttributeValue type"
            ));
        }
    }
    Ok(())
}

/// How a legacy comparison given the wrong number of `AttributeValueList` entries is
/// refused: AWS's sentence for that defect, over the operator the entry named.
///
/// "AttributeValueList - One or more values to evaluate against the supplied attribute.
/// The number of values in the list depends on the ComparisonOperator being used"
/// (`Condition$AttributeValueList`), so the count is part of the operator's own
/// definition — which is why the refusal names the operator and neither the count it
/// takes nor the count that was sent.
pub(super) fn invalid_argument_count(operator: &str) -> String {
    format!(
        "{INVALID_PARAMETER_CLAUSE}: Invalid number of argument(s) for the {operator} \
         ComparisonOperator"
    )
}

/// Hold a legacy condition MAP to what one request may ask the service to evaluate for
/// every item it reads, and settle every fact of the request its entries carry: how many
/// entries there are ([`MAX_LEGACY_CONDITIONS`]), whether each names a comparison this
/// service serves with the operands that comparison takes
/// ([`check_legacy_comparison`]), and whether the `ConditionalOperator` combining them is
/// one the model declares.
///
/// Settled ONCE, before anything is read, so the work the evaluator below does per item
/// is a function of this service's constants rather than of the request body.
pub fn check_legacy_conditions(
    conditions: &HashMap<String, Condition>,
    conditional_operator: Option<&str>,
) -> Result<(), RequestError> {
    if conditions.len() > MAX_LEGACY_CONDITIONS {
        return Err(RequestError::Validation(format!(
            "One or more parameter values were invalid: a filter may state at most \
             {MAX_LEGACY_CONDITIONS} conditions, got {}",
            conditions.len()
        )));
    }
    // The attribute NAME each entry is keyed by is the name an item is read at, so it is
    // held to the bound an attribute name has: the model declares it on the shape the key
    // targets, and no walk ahead of this reader can state it, because the validating walk
    // reports a map's KEY as a segment of the path it names a violation under rather than as
    // a value of its own ([`carries_an_over_long_attribute_name`], the same rule over the
    // keys a VALUE nests).
    for name in conditions.keys() {
        if name.len() > MAX_ATTRIBUTE_NAME_BYTES {
            return Err(RequestError::Validation(attribute_name_too_large_message(
                None,
            )));
        }
    }
    for condition in conditions.values() {
        check_legacy_comparison(
            &condition.comparison_operator,
            legacy_operands(condition),
        )?;
    }
    conditional_operator_any(conditional_operator)?;
    Ok(())
}

/// The operands one entry of a legacy condition map compares against.
pub fn legacy_operands(condition: &Condition) -> &[AttributeValue] {
    condition
        .attribute_value_list
        .as_deref()
        .unwrap_or_default()
}

/// The comparator whose operand count the REQUEST chooses, which is the one a read
/// indexes before it walks any item ([`LegacyMembership`]) — and the word the expression
/// language spells that same membership test with, which is what its refusals name
/// ([`non_distinct_operands`]).
pub(super) const IN_OPERATOR: &str = "IN";

/// The membership tests of one legacy condition map, indexed ONCE per request.
///
/// `IN` asks whether the stored value is one of the values its entry listed, and that
/// entry is evaluated FOR EVERY ITEM A READ EVALUATES, over a list whose length the
/// request chooses ("AttributeValueList can contain one or more AttributeValue elements of
/// type String, Number, or Binary" — `ComparisonOperator`, the `IN` entry, which states no
/// ceiling). So the list is turned into a lookup here, before the walk starts: what one
/// `IN` entry then costs per item is ONE key of the stored value, whatever the list's
/// length, rather than a scan of the list.
#[derive(Debug, Default)]
pub struct LegacyMembership {
    /// One set of operand keys per attribute whose entry named `IN`.
    pub(super) by_attribute: HashMap<String, HashSet<String>>,
}

impl LegacyMembership {
    /// Index every `IN` entry of `conditions`.
    ///
    /// Built where the filter is resolved, which is after the entries have been held to the
    /// rules that bind their lists ([`check_legacy_operand_list`]), so every operand keyed
    /// here is one of the types that comparison compares.
    pub fn of(conditions: &HashMap<String, Condition>) -> Self {
        Self {
            by_attribute: conditions
                .iter()
                .filter(|(_, condition)| condition.comparison_operator == IN_OPERATOR)
                .map(|(name, condition)| {
                    (
                        name.clone(),
                        legacy_operands(condition)
                            .iter()
                            .map(membership_key)
                            .collect::<HashSet<String>>(),
                    )
                })
                .collect(),
        }
    }

    /// Whether the value an item holds for `name` is one the `IN` entry over that name
    /// listed. An item that holds no value there is compared against nothing, which is the
    /// answer every comparator but `NE` gives it ([`legacy_comparison_matches`]).
    pub(super) fn holds(&self, name: &str, held: Option<&AttributeValue>) -> bool {
        let Some(held) = held else {
            return false;
        };
        self.by_attribute
            .get(name)
            .is_some_and(|keys| keys.contains(&membership_key(held)))
    }
}

/// The key a value is looked up by in the index above: its `AttributeValue` type tag and
/// its own text, which is how [`attribute_values_equal`] tells two values apart — by type
/// first, and two `N` spellings of one number by the number they denote, so `6` and `6.0`
/// key alike ([`canonical_number`]).
///
/// A type whose equality is not textual — a set, a list, a map — keys to its tag alone,
/// which no operand of an `IN` comparison can key to: those operands are `S`, `N` or `B`
/// values, so a stored value of any other type matches none of them, which is the answer
/// the comparison gives it either way.
pub(super) fn membership_key(value: &AttributeValue) -> String {
    match value {
        AttributeValue::S(text) => format!("S:{text}"),
        AttributeValue::N(text) => format!("N:{}", canonical_number(text)),
        AttributeValue::B(bytes) => format!("B:{}", ::aws_smithy_types::base64::encode(bytes)),
        AttributeValue::Bool(flag) => format!("BOOL:{flag}"),
        AttributeValue::Null(flag) => format!("NULL:{flag}"),
        other => format!("{}:", attribute_value_type(other)),
    }
}

/// Whether a legacy CONDITION MAP — the pre-expression filter surface (`ScanFilter`,
/// `QueryFilter`) — selects `item`, its entries combined by `ConditionalOperator`.
///
/// Each entry is read by the same comparison the legacy conditional writes are read by
/// ([`legacy_comparison_matches`]), which is what makes a `NOT_NULL` or `NULL` entry
/// select exactly the items `attribute_exists` and `attribute_not_exists` select: both
/// forms ask one question of one attribute, through one evaluator. The one entry read
/// another way is `IN`, whose operand list is indexed once per request and probed here
/// ([`LegacyMembership`]) — the same question, asked at a cost that does not depend on how
/// many values the entry listed.
///
/// The walk stops as soon as the answer is settled — the first entry that fails an `AND`
/// or holds for an `OR` — so what one item costs is bounded by the first decisive entry
/// rather than by the whole map. Whether an entry is one this service can evaluate at all
/// is settled before any item is read ([`check_legacy_conditions`]), so stopping early
/// cannot hide a refusal the request had coming.
pub fn legacy_conditions_match(
    conditions: &HashMap<String, Condition>,
    conditional_operator: Option<&str>,
    membership: &LegacyMembership,
    item: &Item,
) -> Result<bool, RequestError> {
    let any = conditional_operator_any(conditional_operator)?;
    for (name, condition) in conditions {
        if condition.comparison_operator == IN_OPERATOR {
            if membership.holds(name, item.get(name)) == any {
                return Ok(any);
            }
            continue;
        }
        let holds = legacy_comparison_matches(
            &condition.comparison_operator,
            item.get(name),
            legacy_operands(condition),
        )?;
        if holds == any {
            return Ok(any);
        }
    }
    Ok(!any)
}
