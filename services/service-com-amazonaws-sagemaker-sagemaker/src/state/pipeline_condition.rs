// SPDX-License-Identifier: CC0-1.0

//! The CONDITION slice of a pipeline run: evaluating the conditions a condition step
//! declares, so the run hands control to the branch those conditions select.
//!
//! A condition step's whole work is a comparison of values the definition names — it
//! starts no job and needs no runtime — so it is performed HERE, by evaluating the
//! comparison the definition wrote against the values this run carries. The condition
//! vocabulary is the one SageMaker Pipelines publishes: `Equals`, `GreaterThan`,
//! `GreaterThanOrEqualTo`, `LessThan`, `LessThanOrEqualTo`, `In`, `Not` and `Or`, each
//! serialised as a `Type` beside the operands it compares.
//!
//! Every evaluation can answer "I cannot tell": a condition of a kind this reading does not
//! know, an operand that references something no value is resolved for, or a comparison
//! between values that cannot be ordered. The answer is then [`None`], and the run performs
//! that condition step for nothing rather than picking a branch it has not established —
//! inventing an outcome would run one branch of the caller's pipeline on a coin toss.

use super::*;

/// The key a condition object names the comparison it makes under.
const CONDITION_TYPE_KEY: &str = "Type";

/// The two operands the comparing conditions carry.
const CONDITION_LEFT_KEY: &str = "LeftValue";
/// See [`CONDITION_LEFT_KEY`].
const CONDITION_RIGHT_KEY: &str = "RightValue";

/// The value an `In` condition looks for, and the list it looks for it in.
const CONDITION_QUERY_KEY: &str = "QueryValue";
/// See [`CONDITION_QUERY_KEY`].
const CONDITION_VALUES_KEY: &str = "Values";

/// The condition a `Not` negates.
const CONDITION_EXPRESSION_KEY: &str = "Expression";

/// The conditions an `Or` holds.
const CONDITION_CONDITIONS_KEY: &str = "Conditions";

/// The `Type`s a condition declares, as SageMaker Pipelines publishes them (the condition
/// types its Python SDK's `ConditionTypeEnum` serialises: `Equals`, `GreaterThan`,
/// `GreaterThanOrEqualTo`, `LessThan`, `LessThanOrEqualTo`, `In`, `Not`, `Or`). Named here
/// so the evaluation below compares no bare string literal.
const CONDITION_EQUALS: &str = "Equals";
/// See [`CONDITION_EQUALS`].
const CONDITION_GREATER_THAN: &str = "GreaterThan";
/// See [`CONDITION_EQUALS`].
const CONDITION_GREATER_THAN_OR_EQUAL_TO: &str = "GreaterThanOrEqualTo";
/// See [`CONDITION_EQUALS`].
const CONDITION_LESS_THAN: &str = "LessThan";
/// See [`CONDITION_EQUALS`].
const CONDITION_LESS_THAN_OR_EQUAL_TO: &str = "LessThanOrEqualTo";
/// See [`CONDITION_EQUALS`].
const CONDITION_IN: &str = "In";
/// See [`CONDITION_EQUALS`].
const CONDITION_NOT: &str = "Not";
/// See [`CONDITION_EQUALS`].
const CONDITION_OR: &str = "Or";

/// How deep a condition may nest its sub-conditions before this reading stops: a `Not` of
/// an `Or` of a `Not` is ordinary, a tower of them is a document no pipeline author writes
/// and the walk answers "cannot tell" for it rather than recursing without bound.
const MAX_CONDITION_DEPTH: usize = 16;

/// Whether every condition in `conditions` holds for a run carrying `parameters` — the
/// outcome a condition step hands control to its branches by — or `None` where this reading
/// cannot establish it.
///
/// A condition step's conditions are a LIST, and the step takes its `IfSteps` branch when
/// all of them hold (one condition that does not hold decides the step, whatever the rest
/// say), which is why a single unestablished condition only matters while no condition has
/// already answered `false`.
#[must_use]
pub fn pipeline_conditions_hold(
    conditions: &::serde_json::Value,
    parameters: &BTreeMap<String, ::serde_json::Value>,
) -> Option<bool> {
    let entries = conditions.as_array()?;
    // A condition step declaring an empty condition list establishes nothing to decide by.
    if entries.is_empty() {
        return None;
    }
    let mut established = true;
    for condition in entries {
        match condition_holds(condition, parameters, 1) {
            Some(false) => return Some(false),
            Some(true) => {}
            None => established = false,
        }
    }
    established.then_some(true)
}

/// Whether ONE condition holds: see [`pipeline_conditions_hold`].
fn condition_holds(
    condition: &::serde_json::Value,
    parameters: &BTreeMap<String, ::serde_json::Value>,
    depth: usize,
) -> Option<bool> {
    if depth > MAX_CONDITION_DEPTH {
        return None;
    }
    let kind = condition
        .get(CONDITION_TYPE_KEY)
        .and_then(::serde_json::Value::as_str)?;
    match kind {
        CONDITION_EQUALS => {
            let (left, right) = operands(condition, parameters)?;
            Some(values_are_equal(&left, &right))
        }
        CONDITION_GREATER_THAN => Some(comparison(condition, parameters)?.is_gt()),
        CONDITION_GREATER_THAN_OR_EQUAL_TO => Some(comparison(condition, parameters)?.is_ge()),
        CONDITION_LESS_THAN => Some(comparison(condition, parameters)?.is_lt()),
        CONDITION_LESS_THAN_OR_EQUAL_TO => Some(comparison(condition, parameters)?.is_le()),
        CONDITION_IN => {
            let query = resolve_operand(condition.get(CONDITION_QUERY_KEY)?, parameters)?;
            let values = condition
                .get(CONDITION_VALUES_KEY)
                .and_then(::serde_json::Value::as_array)?;
            // One unresolved entry leaves the membership unestablished only while nothing
            // else in the list already matches.
            let mut established = true;
            for value in values {
                match resolve_operand(value, parameters) {
                    Some(resolved) if values_are_equal(&query, &resolved) => return Some(true),
                    Some(_) => {}
                    None => established = false,
                }
            }
            established.then_some(false)
        }
        CONDITION_NOT => {
            let negated = condition_holds(
                condition.get(CONDITION_EXPRESSION_KEY)?,
                parameters,
                depth + 1,
            )?;
            Some(!negated)
        }
        CONDITION_OR => {
            let entries = condition
                .get(CONDITION_CONDITIONS_KEY)
                .and_then(::serde_json::Value::as_array)?;
            if entries.is_empty() {
                return None;
            }
            let mut established = true;
            for entry in entries {
                match condition_holds(entry, parameters, depth + 1) {
                    Some(true) => return Some(true),
                    Some(false) => {}
                    None => established = false,
                }
            }
            established.then_some(false)
        }
        // A condition kind this reading does not know decides nothing: see this module's
        // header.
        _ => None,
    }
}

/// The two operands a comparing condition carries, each resolved.
fn operands(
    condition: &::serde_json::Value,
    parameters: &BTreeMap<String, ::serde_json::Value>,
) -> Option<(::serde_json::Value, ::serde_json::Value)> {
    let left = resolve_operand(condition.get(CONDITION_LEFT_KEY)?, parameters)?;
    let right = resolve_operand(condition.get(CONDITION_RIGHT_KEY)?, parameters)?;
    Some((left, right))
}

/// How the two operands of an ORDERING condition compare, or `None` where they cannot be
/// ordered: an order is established between two numbers (a numeric string included, since a
/// pipeline parameter's value travels as text) and between two strings, and between nothing
/// else.
fn comparison(
    condition: &::serde_json::Value,
    parameters: &BTreeMap<String, ::serde_json::Value>,
) -> Option<::std::cmp::Ordering> {
    let (left, right) = operands(condition, parameters)?;
    match (numeric(&left), numeric(&right)) {
        (Some(left), Some(right)) => left.partial_cmp(&right),
        _ => match (text(&left), text(&right)) {
            (Some(left), Some(right)) => Some(left.cmp(&right)),
            _ => None,
        },
    }
}

/// Whether two resolved operands are the same value: two numbers are equal when they are
/// numerically equal (so a parameter's `"2"` equals a definition's `2`), and two other
/// scalars when their text is the same. Two values neither of which is a scalar are not
/// compared at all.
fn values_are_equal(left: &::serde_json::Value, right: &::serde_json::Value) -> bool {
    match (numeric(left), numeric(right)) {
        (Some(left), Some(right)) => left == right,
        _ => match (text(left), text(right)) {
            (Some(left), Some(right)) => left == right,
            _ => false,
        },
    }
}

/// `value` as a number, where it is one: a JSON number, or a string that spells one.
fn numeric(value: &::serde_json::Value) -> Option<f64> {
    match value {
        ::serde_json::Value::Number(number) => number.as_f64(),
        ::serde_json::Value::String(text) => text.trim().parse::<f64>().ok(),
        _ => None,
    }
}

/// `value` as the text a comparison reads it as, where it is a scalar; `None` for a list or
/// an object, which no condition of this vocabulary compares.
fn text(value: &::serde_json::Value) -> Option<String> {
    match value {
        ::serde_json::Value::String(text) => Some(text.clone()),
        ::serde_json::Value::Bool(flag) => Some(flag.to_string()),
        ::serde_json::Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

/// One operand, with a property reference resolved to the value this run carries for it.
///
/// A definition writes a reference as the schema's `{"Get": "<reference>"}`, and the only
/// references a run has values for are its own parameters (`Parameters.<name>`): a reference
/// to a step's properties or to the execution names something no step of this run produced,
/// so it resolves to nothing and the condition it appears in establishes nothing.
fn resolve_operand(
    operand: &::serde_json::Value,
    parameters: &BTreeMap<String, ::serde_json::Value>,
) -> Option<::serde_json::Value> {
    let Some(members) = operand.as_object() else {
        return Some(operand.clone());
    };
    match members
        .get(PIPELINE_PROPERTY_REFERENCE_KEY)
        .and_then(::serde_json::Value::as_str)
    {
        Some(reference) => {
            let parameter = reference.strip_prefix(PIPELINE_PARAMETER_REFERENCE_PREFIX)?;
            parameters.get(parameter).cloned()
        }
        // An object that is no reference is a value of its own, which no condition of this
        // vocabulary compares — `values_are_equal` and `comparison` answer that.
        None => Some(operand.clone()),
    }
}

/// The values a run resolves its definition's parameters to: the defaults the definition
/// declared, each overridden by the value the start call assigned it.
///
/// `Parameter$Name`'s doc — "This parameter name must match a named parameter in the
/// pipeline definition" — is what makes this one map: the start call may only assign
/// parameters the definition declares, so an assignment always replaces a declared
/// parameter's default rather than adding a name of its own. A parameter the definition
/// declared without a default and the run assigned nothing to is in neither, so a condition
/// referencing it establishes nothing.
#[must_use]
pub fn pipeline_parameter_values(
    definition: &str,
    assigned: &[Parameter],
) -> BTreeMap<String, ::serde_json::Value> {
    let mut values = pipeline_definition_parameter_defaults(definition);
    for parameter in assigned {
        values.insert(
            parameter.name.clone(),
            ::serde_json::Value::String(parameter.value.clone()),
        );
    }
    values
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A parameter map from `(name, value)` pairs, the values as the definition wrote them.
    fn parameters(pairs: &[(&str, ::serde_json::Value)]) -> BTreeMap<String, ::serde_json::Value> {
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_string(), value.clone()))
            .collect()
    }

    /// A reference to the pipeline parameter `name`, as a definition writes one.
    fn reference(name: &str) -> ::serde_json::Value {
        ::serde_json::json!({ PIPELINE_PROPERTY_REFERENCE_KEY: format!("Parameters.{name}") })
    }

    /// RULE: a condition step's outcome is the conjunction of the conditions it declares,
    /// each evaluated over the values the run carries — and a condition this reading cannot
    /// establish leaves the outcome unknown rather than guessed.
    #[test]
    fn a_conditions_outcome_follows_the_values_the_run_carries() {
        let values = parameters(&[
            ("Threshold", ::serde_json::json!(10)),
            ("Name", ::serde_json::json!("alpha")),
            ("Count", ::serde_json::json!("7")),
        ]);
        // Rows span the published condition vocabulary, the operand forms (literal,
        // reference, numeric string) and the three answers.
        let rows: Vec<(&str, ::serde_json::Value, Option<bool>)> = vec![
            (
                "equal literals",
                ::serde_json::json!([{ "Type": "Equals", "LeftValue": 1, "RightValue": 1 }]),
                Some(true),
            ),
            (
                "unequal literals",
                ::serde_json::json!([{ "Type": "Equals", "LeftValue": 1, "RightValue": 2 }]),
                Some(false),
            ),
            (
                "a parameter equal to a literal",
                ::serde_json::json!([
                    { "Type": "Equals", "LeftValue": reference("Threshold"), "RightValue": 10 }
                ]),
                Some(true),
            ),
            (
                "a parameter whose text spells the number",
                ::serde_json::json!([
                    { "Type": "Equals", "LeftValue": reference("Count"), "RightValue": 7 }
                ]),
                Some(true),
            ),
            (
                "a string parameter",
                ::serde_json::json!([
                    { "Type": "Equals", "LeftValue": reference("Name"), "RightValue": "alpha" }
                ]),
                Some(true),
            ),
            (
                "greater than",
                ::serde_json::json!([
                    { "Type": "GreaterThan", "LeftValue": reference("Threshold"), "RightValue": 9 }
                ]),
                Some(true),
            ),
            (
                "greater than or equal to, at the bound",
                ::serde_json::json!([
                    {
                        "Type": "GreaterThanOrEqualTo",
                        "LeftValue": reference("Threshold"),
                        "RightValue": 10,
                    }
                ]),
                Some(true),
            ),
            (
                "less than",
                ::serde_json::json!([
                    { "Type": "LessThan", "LeftValue": reference("Threshold"), "RightValue": 9 }
                ]),
                Some(false),
            ),
            (
                "less than or equal to, at the bound",
                ::serde_json::json!([
                    {
                        "Type": "LessThanOrEqualTo",
                        "LeftValue": reference("Threshold"),
                        "RightValue": 10,
                    }
                ]),
                Some(true),
            ),
            (
                "in a list",
                ::serde_json::json!([
                    {
                        "Type": "In",
                        "QueryValue": reference("Name"),
                        "Values": ["beta", "alpha"],
                    }
                ]),
                Some(true),
            ),
            (
                "in no list entry",
                ::serde_json::json!([
                    { "Type": "In", "QueryValue": reference("Name"), "Values": ["beta"] }
                ]),
                Some(false),
            ),
            (
                "not",
                ::serde_json::json!([
                    {
                        "Type": "Not",
                        "Expression": { "Type": "Equals", "LeftValue": 1, "RightValue": 2 },
                    }
                ]),
                Some(true),
            ),
            (
                "or, one holding",
                ::serde_json::json!([
                    {
                        "Type": "Or",
                        "Conditions": [
                            { "Type": "Equals", "LeftValue": 1, "RightValue": 2 },
                            { "Type": "Equals", "LeftValue": 2, "RightValue": 2 },
                        ],
                    }
                ]),
                Some(true),
            ),
            (
                "every condition of a list holding",
                ::serde_json::json!([
                    { "Type": "Equals", "LeftValue": 1, "RightValue": 1 },
                    { "Type": "LessThan", "LeftValue": 1, "RightValue": 2 },
                ]),
                Some(true),
            ),
            (
                "one condition of a list not holding",
                ::serde_json::json!([
                    { "Type": "Equals", "LeftValue": 1, "RightValue": 1 },
                    { "Type": "LessThan", "LeftValue": 3, "RightValue": 2 },
                ]),
                Some(false),
            ),
            (
                "a condition kind this reading does not know",
                ::serde_json::json!([{ "Type": "Matches", "LeftValue": 1, "RightValue": 1 }]),
                None,
            ),
            (
                "a reference to a parameter the run carries no value for",
                ::serde_json::json!([
                    { "Type": "Equals", "LeftValue": reference("Unset"), "RightValue": 1 }
                ]),
                None,
            ),
            (
                "a reference to something other than a parameter",
                ::serde_json::json!([
                    {
                        "Type": "Equals",
                        "LeftValue": { "Get": "Steps.Train.Outcome" },
                        "RightValue": 1,
                    }
                ]),
                None,
            ),
            (
                "values that cannot be ordered",
                ::serde_json::json!([
                    { "Type": "GreaterThan", "LeftValue": ["a"], "RightValue": 1 }
                ]),
                None,
            ),
            (
                "a condition list holding nothing",
                ::serde_json::json!([]),
                None,
            ),
            (
                "an unestablished condition beside one that does not hold",
                ::serde_json::json!([
                    { "Type": "Equals", "LeftValue": reference("Unset"), "RightValue": 1 },
                    { "Type": "Equals", "LeftValue": 1, "RightValue": 2 },
                ]),
                Some(false),
            ),
        ];

        for (case, conditions, expected) in rows {
            let actual = pipeline_conditions_hold(&conditions, &values);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?} for {conditions}, got {actual:?}"
            );
        }
    }

    /// RULE: a run resolves its parameters to the definition's declared defaults, each
    /// overridden by the value the start call assigned it.
    #[test]
    fn a_runs_parameter_values_are_the_defaults_the_start_overrode() {
        let definition = ::serde_json::json!({
            "Version": PIPELINE_DEFINITION_VERSION,
            "Parameters": [
                { "Name": "WithDefault", "Type": "String", "DefaultValue": "declared" },
                { "Name": "Numeric", "Type": "Integer", "DefaultValue": 4 },
                { "Name": "WithoutDefault", "Type": "String" },
            ],
            "Steps": [],
        })
        .to_string();
        let assigned = vec![Parameter {
            name: "WithDefault".to_string(),
            value: "assigned".to_string(),
        }];

        let values = pipeline_parameter_values(&definition, &assigned);
        assert_eq!(
            values.get("WithDefault"),
            Some(&::serde_json::json!("assigned")),
            "case assigned: the start's value overrides the default, got {values:?}"
        );
        assert_eq!(
            values.get("Numeric"),
            Some(&::serde_json::json!(4)),
            "case default-kept: a parameter the start left alone keeps its default, got \
             {values:?}"
        );
        assert_eq!(
            values.get("WithoutDefault"),
            None,
            "case no-default: a parameter with neither a default nor an assignment \
             resolves to nothing, got {values:?}"
        );
    }
}
