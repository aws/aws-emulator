// SPDX-License-Identifier: CC0-1.0

//! What a parameter VALUE in force on a stack operation is held to: the constraints the
//! template's own declaration of that parameter states.
//!
//! A template's `Parameters` section is the template's INPUT CONTRACT — "you can specify
//! allowed values and constraints for each parameter" — and CloudFormation enforces that
//! contract when a stack operation submits values: a value its declaration does not admit
//! is refused with `ValidationError` before anything is provisioned, so a bad value never
//! reaches a resource. The declaration states the contract in six members, and every one of
//! them is a rule about a VALUE:
//!
//! * `Type` — `Number` is "an integer or float", so a value that is neither is no value of
//!   that parameter at all;
//! * `MinValue` / `MaxValue` — "the smallest/largest numeric value you want to allow for
//!   Number types";
//! * `MinLength` / `MaxLength` — "the smallest/largest number of characters you want to
//!   allow for String types";
//! * `AllowedValues` — "an array containing the list of values allowed for the parameter";
//! * `AllowedPattern` — "a regular expression that represents the patterns to allow for
//!   String types", which the WHOLE value has to match.
//!
//! A LIST-typed parameter states several values in one entry (`CommaDelimitedList` is "an
//! array of literal strings that are separated by commas", and so is every `List<…>` type),
//! and each of them is a value of that parameter: every one is held to the declaration.
//!
//! `ConstraintDescription` is the one member that is not a rule but the WORDING of one: "a
//! string that explains a constraint when the constraint is violated", which CloudFormation
//! reports in place of the constraint's own default clause — the member exists so a template
//! author can "display [a] customized error message" instead of the default one, so a
//! declaration that states it has its sentence reported for whichever of its constraints the
//! value failed.
//!
//! The SET of parameters is part of that same contract, and is held to it before any one
//! value is judged ([`set_refusal`]): a declaration with no `Default` that nothing puts a
//! value in force for, and a key the request states that the section declares nothing for,
//! are both refused.
//!
//! The values this pass reads are the ones IN FORCE — what the request supplied, over the
//! template's own `Default` for a parameter it passed none for — so a declaration whose own
//! default its constraints do not admit is refused on the same rule, which is what a
//! template submitted with no values at all is held to.

use ::std::collections::{BTreeMap, BTreeSet};

use super::template::{self, DeclaredParameter};

/// `Some(message)` when the parameter set a stack operation puts in force does not MATCH the
/// set the template declares; `None` when the two agree.
///
/// The `Parameters` section names exactly which parameters a stack operation states values
/// for, so the SET itself is part of the template's input contract and is held to it before
/// any value is judged:
///
/// * a declared parameter is answered either from a value in force for it or from the
///   declaration's own `Default` — "a value of the appropriate type for the template to use if
///   no value is specified" — so a declaration stating no `Default` with nothing in force for
///   it has no value at all, and every such parameter is named together in the one refusal
///   saying they must be given values;
/// * a value the request STATES for a key the section declares no parameter for answers
///   nothing in the template — only a declared parameter can be referenced — and is refused
///   naming those keys.
///
/// The two sets are different questions and so are read from different inputs: what is in
/// force is everything the operation resolved (what it states, each declaration's own
/// `Default`, and on an update the value the stack already carries), while an undeclared key
/// is one THIS request named — a value carried over from a stack whose template declared it
/// is not a key this request stated.
///
/// The unsatisfied keys are named in the order the section declares them and the undeclared
/// ones in name order, so one body and one set of values always answer the same sentence.
pub fn set_refusal(
    body: &str,
    in_force: &BTreeSet<&str>,
    stated: &BTreeSet<&str>,
) -> Option<String> {
    let declared = template::parse(body)?;
    let unsatisfied: Vec<&str> = declared
        .parameters
        .iter()
        .filter(|parameter| {
            parameter.default.is_none() && !in_force.contains(parameter.key.as_str())
        })
        .map(|parameter| parameter.key.as_str())
        .collect();
    if !unsatisfied.is_empty() {
        return Some(format!(
            "Parameters: [{}] must have values",
            unsatisfied.join(", ")
        ));
    }
    let keys: BTreeSet<&str> = declared
        .parameters
        .iter()
        .map(|parameter| parameter.key.as_str())
        .collect();
    let undeclared: Vec<&str> = stated
        .iter()
        .copied()
        .filter(|key| !keys.contains(key))
        .collect();
    if !undeclared.is_empty() {
        return Some(format!(
            "Parameters: [{}] do not exist in the template",
            undeclared.join(", ")
        ));
    }
    None
}

/// `Some(message)` for the first value in force its parameter's declaration does not admit,
/// worded as CloudFormation words that refusal; `None` when every value satisfies the
/// declaration it is for.
///
/// The value IN FORCE for a declared parameter is the one `values` carries for it, else the
/// declaration's own `Default` ("a value of the appropriate type for the template to use if
/// no value is specified") — so this ONE pass holds a supplied value and a declared default
/// to the same constraints, and a template submitted with no values at all is held to its
/// own defaults.
///
/// The parameters are walked in the order the template reader yields them, so one body and
/// one set of values always answer the same refusal. A value for a key the template declares
/// no parameter for is not this rule's business (nothing declares a constraint for it), and
/// neither is a body this reader will not read at all — that is a template-FORM question the
/// submitting operation answers on its own terms.
///
/// # The work one request can ask for
///
/// Both inputs are the caller's, so the pass is bounded in the two directions a caller can
/// grow it. A declared `AllowedPattern` is compiled ONCE per parameter, before its values are
/// judged, so the number of compiles is the number of declared parameters — bounded by the
/// documented per-template parameter quota ([`template::MAX_PARAMETERS_PER_TEMPLATE`]) —
/// however many values one entry carries; and at most [`MAX_JUDGED_VALUES`] values of one
/// entry are judged, so the per-parameter work is bounded by a constant rather than by the
/// length of a value the request chose. Each match is linear in the value, since the engine
/// is the linear-time one.
pub fn refusal(body: &str, values: &BTreeMap<String, String>) -> Option<String> {
    let declared = template::parse(body)?;
    declared.parameters.iter().find_map(|parameter| {
        let value = values
            .get(&parameter.key)
            .or(parameter.default.as_ref())?;
        // Compiled once for this declaration, not once per value it carries.
        let pattern = parameter.allowed_pattern.as_deref().map(|pattern| Required {
            source: pattern,
            expression: compiled_pattern(pattern),
        });
        stated_values(parameter, value).find_map(|one| violation(parameter, pattern.as_ref(), one))
    })
}

/// The `AllowedPattern` a declaration states, as the expression its values are matched
/// against plus the source text a refusal quotes.
struct Required<'a> {
    source: &'a str,
    /// `None` for an expression this engine will not compile, which constrains nothing: a
    /// value is never refused because the template's own expression could not be read.
    expression: Option<::regex::Regex>,
}

/// How many values of ONE entry this pass judges.
///
/// A list-typed parameter's value is "an array of literal strings that are separated by
/// commas", and how many it carries is the REQUEST's to choose, so the pass is held to a
/// ceiling rather than to the request's own size. Chosen: far more values than a template's
/// parameter is given in practice, and a constant, so the work one request can ask of this
/// pass is bounded by it times the per-template parameter quota. A value past the ceiling is
/// left to the deployment that uses it rather than judged here.
pub const MAX_JUDGED_VALUES: usize = 1_000;

/// The values one entry puts in force for `parameter`: the whole text for a scalar
/// parameter, and each comma-separated entry for a list-typed one, whose value IS that
/// array — at most [`MAX_JUDGED_VALUES`] of them, which is what bounds this pass against an
/// entry carrying as many values as the request's own size allows.
fn stated_values<'a>(
    parameter: &DeclaredParameter,
    value: &'a str,
) -> impl Iterator<Item = &'a str> {
    let values: Vec<&str> = if is_list(parameter) {
        value.split(',').map(str::trim).take(MAX_JUDGED_VALUES).collect()
    } else {
        vec![value]
    };
    values.into_iter()
}

/// Whether the declaration's `Type` makes its value an ARRAY of values — `CommaDelimitedList`
/// and every `List<…>` type, read the one way this crate reads list-ness.
fn is_list(parameter: &DeclaredParameter) -> bool {
    parameter
        .declared_type
        .as_deref()
        .is_some_and(|declared_type| declared_type.contains("List"))
}

/// Whether the declaration's `Type` makes its value a NUMBER: "Number: an integer or float",
/// and `List<Number>`, whose elements are each one of those.
fn is_number(parameter: &DeclaredParameter) -> bool {
    parameter
        .declared_type
        .as_deref()
        .is_some_and(|declared_type| declared_type == "Number" || declared_type == "List<Number>")
}

/// `Some(message)` for ONE value of `parameter` the declaration does not admit.
///
/// The constraints are weighed in the order a value fails them: what the value IS (its
/// type), then how large or long it is, then whether it is one of the admitted values, then
/// the pattern the whole value has to match — so the clause reported is about the first
/// requirement the value does not meet.
fn violation(
    parameter: &DeclaredParameter,
    pattern: Option<&Required<'_>>,
    value: &str,
) -> Option<String> {
    if is_number(parameter) {
        // "Number: an integer or float. CloudFormation validates the parameter value as a
        // number." The clause is a TERMINATED sentence, as the service words this one
        // (recorded behaviour): the declared type is not a bound the clause has to name, so
        // the sentence ends there.
        // A spelling that is NEITHER an integer nor a float — a word, or one of the
        // non-finite spellings a float syntax admits — is no value of that parameter.
        let number = match value.trim().parse::<f64>() {
            Ok(number) if number.is_finite() => number,
            _ => return Some(refused(parameter, "must be a number.".to_string())),
        };
        if let Some(min) = parameter.min_value {
            if number < min {
                return Some(refused(
                    parameter,
                    format!("must be a number not less than {min}"),
                ));
            }
        }
        if let Some(max) = parameter.max_value {
            if number > max {
                return Some(refused(
                    parameter,
                    format!("must be a number not greater than {max}"),
                ));
            }
        }
    } else {
        // The character counts are counted in CHARACTERS, which is what the members are
        // documented in ("the smallest number of characters").
        let characters = value.chars().count() as u64;
        if let Some(min) = parameter.min_length {
            if characters < min {
                return Some(refused(
                    parameter,
                    format!("must have length not less than {min}"),
                ));
            }
        }
        if let Some(max) = parameter.max_length {
            if characters > max {
                return Some(refused(
                    parameter,
                    format!("must have length not greater than {max}"),
                ));
            }
        }
    }
    if !parameter.allowed_values.is_empty()
        && !parameter
            .allowed_values
            .iter()
            .any(|allowed| allowed == value)
    {
        return Some(refused(
            parameter,
            "must be one of AllowedValues".to_string(),
        ));
    }
    if let Some(pattern) = pattern {
        let matched = match &pattern.expression {
            Some(expression) => expression.is_match(value),
            None => true,
        };
        if !matched {
            return Some(refused(
                parameter,
                format!("must match pattern {}", pattern.source),
            ));
        }
    }
    None
}

/// The refusal for one violated constraint.
///
/// A declaration that states a `ConstraintDescription` has THAT sentence reported, under the
/// frame CloudFormation reports a parameter's own constraint under — the member exists so a
/// template author can "display [a] customized error message" when the constraint is
/// violated, and the author's sentence is what the caller is shown. A declaration that
/// states none is reported by the constraint's own default clause, naming the parameter it
/// is about.
fn refused(parameter: &DeclaredParameter, default_clause: String) -> String {
    match parameter.constraint_description.as_deref() {
        Some(description) => format!(
            "Parameter {} failed to satisfy constraint: {description}",
            parameter.key
        ),
        None => format!("Parameter '{}' {default_clause}", parameter.key),
    }
}

/// The declared `AllowedPattern` as the expression a value is matched against, or `None` for
/// one this engine will not compile.
///
/// "AllowedPattern: a regular expression that represents the patterns to allow" — a
/// declaration states the pattern the value has to be, not one it has to contain, so the
/// expression is anchored at both ends before it is applied (a pattern that anchors itself is
/// unaffected).
///
/// Compiled ONCE per declaration by [`refusal`], never per value: the compile is the costly
/// half, and the number of them is then the number of parameters a template may declare. The
/// expression is the template's own, so its SIZE is bounded by the body a request may submit,
/// and the program it compiles to is held to [`MAX_COMPILED_PATTERN_BYTES`]; the engine is the
/// linear-time one, so applying it costs time linear in the value.
fn compiled_pattern(pattern: &str) -> Option<::regex::Regex> {
    ::regex::RegexBuilder::new(&format!("^(?:{pattern})$"))
        .size_limit(MAX_COMPILED_PATTERN_BYTES)
        .dfa_size_limit(MAX_COMPILED_PATTERN_BYTES)
        .build()
        .ok()
}

/// The most compiled program one declared `AllowedPattern` may become: past it the
/// declaration constrains nothing rather than costing the process memory a template chose.
///
/// Chosen, and far more than any expression a parameter is really declared with (the engine's
/// own default is a megabyte).
const MAX_COMPILED_PATTERN_BYTES: usize = 64 * 1024;

#[cfg(test)]
mod tests {
    use super::*;

    /// A body declaring one parameter under `declaration`.
    fn body(declaration: &str) -> String {
        format!(
            r#"{{"Parameters":{{"P":{declaration}}},
                "Resources":{{"B":{{"Type":"AWS::S3::Bucket"}}}}}}"#
        )
    }

    /// One value in force for that parameter.
    fn values(value: &str) -> BTreeMap<String, String> {
        BTreeMap::from([("P".to_string(), value.to_string())])
    }

    /// RULE: every constraint a declaration states is a rule about the value, and the clause
    /// reported names the requirement the value does not meet.
    #[test]
    fn each_declared_constraint_is_enforced_on_the_value_in_force() {
        let rows: &[(&str, &str, &str, Option<&str>)] = &[
            (
                "a number below MinValue",
                r#"{"Type":"Number","MinValue":2,"MaxValue":10}"#,
                "1",
                Some("Parameter 'P' must be a number not less than 2"),
            ),
            (
                "a number above MaxValue",
                r#"{"Type":"Number","MinValue":2,"MaxValue":10}"#,
                "11",
                Some("Parameter 'P' must be a number not greater than 10"),
            ),
            (
                "a number at each bound",
                r#"{"Type":"Number","MinValue":2,"MaxValue":10}"#,
                "10",
                None,
            ),
            (
                "a Number parameter given no number at all",
                r#"{"Type":"Number"}"#,
                "eight",
                Some("Parameter 'P' must be a number."),
            ),
            (
                "a float bound stated in a quoted spelling",
                r#"{"Type":"Number","MinValue":"1.5"}"#,
                "1.25",
                Some("Parameter 'P' must be a number not less than 1.5"),
            ),
            (
                "a value outside AllowedValues",
                r#"{"Type":"String","AllowedValues":["dev","prod"]}"#,
                "staging",
                Some("Parameter 'P' must be one of AllowedValues"),
            ),
            (
                "a value AllowedValues admits",
                r#"{"Type":"String","AllowedValues":["dev","prod"]}"#,
                "prod",
                None,
            ),
            (
                "a value the AllowedPattern does not match",
                r#"{"Type":"String","AllowedPattern":"[a-z]+"}"#,
                "Bad1",
                Some("Parameter 'P' must match pattern [a-z]+"),
            ),
            (
                "a value the pattern matches only PART of",
                r#"{"Type":"String","AllowedPattern":"[a-z]+"}"#,
                "abc1",
                Some("Parameter 'P' must match pattern [a-z]+"),
            ),
            (
                "a value the AllowedPattern matches whole",
                r#"{"Type":"String","AllowedPattern":"[a-z]+"}"#,
                "fine",
                None,
            ),
            (
                "a ConstraintDescription in place of the pattern clause",
                r#"{"Type":"String","AllowedPattern":"[a-z]+",
                    "ConstraintDescription":"must be lowercase letters"}"#,
                "Bad1",
                Some("Parameter P failed to satisfy constraint: must be lowercase letters"),
            ),
            (
                "a ConstraintDescription in place of a numeric clause",
                r#"{"Type":"Number","MinValue":2,
                    "ConstraintDescription":"must be at least two"}"#,
                "1",
                Some("Parameter P failed to satisfy constraint: must be at least two"),
            ),
            (
                "a value shorter than MinLength",
                r#"{"Type":"String","MinLength":3,"MaxLength":6}"#,
                "ab",
                Some("Parameter 'P' must have length not less than 3"),
            ),
            (
                "a value longer than MaxLength",
                r#"{"Type":"String","MinLength":3,"MaxLength":6}"#,
                "abcdefg",
                Some("Parameter 'P' must have length not greater than 6"),
            ),
            (
                "a value at each length bound",
                r#"{"Type":"String","MinLength":3,"MaxLength":6}"#,
                "abcdef",
                None,
            ),
            (
                "an element of a list-typed value outside AllowedValues",
                r#"{"Type":"CommaDelimitedList","AllowedValues":["a","b"]}"#,
                "a, c",
                Some("Parameter 'P' must be one of AllowedValues"),
            ),
            (
                "every element of a list-typed value admitted",
                r#"{"Type":"CommaDelimitedList","AllowedValues":["a","b"]}"#,
                "a, b",
                None,
            ),
            (
                "a declaration that constrains nothing",
                r#"{"Type":"String"}"#,
                "anything at all",
                None,
            ),
            (
                "a pattern that does not compile",
                r#"{"Type":"String","AllowedPattern":"[a-"}"#,
                "anything at all",
                None,
            ),
        ];
        for (case, declaration, value, expected) in rows {
            let refusal = refusal(&body(declaration), &values(value));
            assert_eq!(
                refusal.as_deref(),
                *expected,
                "case {case:?}: expected {expected:?}, got {refusal:?}"
            );
        }
    }

    /// RULE: the values held to the declaration are the ones IN FORCE, so a parameter the
    /// request passed no value for is held to the template's own `Default`.
    #[test]
    fn a_declared_default_is_a_value_in_force() {
        let body = body(r#"{"Type":"Number","Default":"1","MinValue":2}"#);
        let defaults: BTreeMap<String, String> =
            crate::state::parameters_in_force(&body, Vec::new())
                .into_iter()
                .map(|parameter| (parameter.key, parameter.value))
                .collect();
        let refusal = refusal(&body, &defaults);
        assert_eq!(
            refusal.as_deref(),
            Some("Parameter 'P' must be a number not less than 2"),
            "case \"a default outside MinValue\": expected the numeric clause, got {refusal:?}"
        );
    }
}
