// SPDX-License-Identifier: CC0-1.0

//! The resource-based policy of an agent resource: the document `PutAgentProfilePolicy`
//! and `PutAgentRecommendationPolicy` attach, and the rule both of them hold it to.
//!
//! Both operations document the same requirement — "the policy must be valid IAM JSON with
//! Allow-only statements and no negation condition operators" — so the check lives here
//! once and both handlers apply it, and neither can word the refusal differently or accept
//! a document the other refuses. Everything the requirement does not speak about is the
//! caller's to shape: nothing here reads a statement's `Sid`, `Principal`, `Action`,
//! `Resource` or the values inside a condition clause, and nothing stores anything but the
//! document as sent.

use super::*;

/// The modelled input member a policy finding names
/// (`PutAgentProfilePolicyRequest$policy` / `PutAgentRecommendationPolicyRequest$policy`),
/// as `ValidationExceptionField$Name` reports it.
pub const POLICY_MEMBER: &str = "policy";

/// The one `Effect` a resource policy of this service may state: the requirement is
/// "Allow-only statements", so a statement deciding anything else is refused.
const ALLOW_EFFECT: &str = "Allow";

/// The element a policy document states its statements in.
const STATEMENT_ELEMENT: &str = "Statement";

/// The element a statement narrows its grant with, whose keys are condition operators.
const CONDITION_ELEMENT: &str = "Condition";

/// What marks a condition operator as a NEGATED one: the policy language spells the
/// negation of each comparison by putting `Not` in the operator's name
/// (`StringNotEquals`, `StringNotLike`, `NumericNotEquals`, `DateNotEquals`,
/// `ArnNotEquals`, `ArnNotLike` and the rest of the family), so the family is recognised
/// by that rather than by a catalog copied here, which a new operator would fall out of.
const NEGATION_MARKER: &str = "Not";

/// Why a policy document was refused, and the rule it broke.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourcePolicyRejection {
    /// The document is not JSON, or is JSON that is not a policy object.
    InvalidSyntax,
    /// It carries no `Statement`, or one that is neither a statement nor a list of them.
    MissingStatement,
    /// Its `Statement` list is empty, so the document grants nothing.
    EmptyStatement,
    /// A statement states no `Effect`, or states one that is not `Allow`.
    NotAllowOnly(String),
    /// A statement keys a condition clause by a negated operator.
    NegationConditionOperator(String),
}

impl ResourcePolicyRejection {
    /// The constraint sentence a refusal states, in the form every member-constraint
    /// refusal of this service takes ([`constraint_sentence`]): the rule the document had
    /// to satisfy, and what was found instead where naming it helps the caller fix the
    /// document.
    pub fn constraint(&self) -> String {
        match self {
            Self::InvalidSyntax => {
                "Member must be a valid IAM policy document in JSON".to_string()
            }
            Self::MissingStatement => format!(
                "Member must be a valid IAM policy document in JSON, stating its \
                 statements in {STATEMENT_ELEMENT}"
            ),
            Self::EmptyStatement => {
                format!("Member must state at least one statement in {STATEMENT_ELEMENT}")
            }
            Self::NotAllowOnly(effect) => format!(
                "Member must state {ALLOW_EFFECT}-only statements, got Effect {effect}"
            ),
            Self::NegationConditionOperator(operator) => format!(
                "Member must not state a negation condition operator, got {operator}"
            ),
        }
    }
}

/// Check `document` against the rule both policy puts document, naming the first rule it
/// breaks.
///
/// A policy document is a JSON object whose `Statement` is one statement or a list of
/// them; each statement's `Effect` is `Allow`, and a statement that narrows its grant with
/// a `Condition` keys every clause by an operator that is not a negated one. The modelled
/// `ResourcePolicy` `@length` and `@pattern` are the request boundary's, so a document
/// reaching here already satisfies both.
pub fn validate_resource_policy(document: &str) -> Result<(), ResourcePolicyRejection> {
    let parsed: ::serde_json::Value =
        ::serde_json::from_str(document).map_err(|_| ResourcePolicyRejection::InvalidSyntax)?;
    let members = parsed
        .as_object()
        .ok_or(ResourcePolicyRejection::InvalidSyntax)?;

    // "Statement — the main element for a policy": one statement object, or a list of
    // them. A document that states its statements any other way is not a policy document.
    let statements: Vec<&::serde_json::Value> = match members.get(STATEMENT_ELEMENT) {
        // One statement object: bound by the pattern itself, so nothing has to look the
        // element up a second time.
        Some(statement @ ::serde_json::Value::Object(_)) => vec![statement],
        Some(::serde_json::Value::Array(entries)) => {
            if entries.is_empty() {
                return Err(ResourcePolicyRejection::EmptyStatement);
            }
            entries.iter().collect()
        }
        _ => return Err(ResourcePolicyRejection::MissingStatement),
    };

    for statement in statements {
        let statement = statement
            .as_object()
            .ok_or(ResourcePolicyRejection::MissingStatement)?;

        // "Allow-only statements": the `Effect` is read without regard to case, as the
        // policy language reads it, and anything but the allowing decision is refused —
        // including a statement that decides nothing at all.
        let effect = statement
            .get("Effect")
            .and_then(::serde_json::Value::as_str)
            .unwrap_or_default();
        if !effect.eq_ignore_ascii_case(ALLOW_EFFECT) {
            return Err(ResourcePolicyRejection::NotAllowOnly(effect.to_string()));
        }

        // "No negation condition operators": a clause keyed by the negation of a
        // comparison is refused, whatever it compares. A `Condition` that is not an object
        // keys no clause at all, so the document is not a policy document.
        if let Some(condition) = statement.get(CONDITION_ELEMENT) {
            let clauses = condition
                .as_object()
                .ok_or(ResourcePolicyRejection::MissingStatement)?;
            if let Some(operator) = clauses.keys().find(|operator| negates(operator)) {
                return Err(ResourcePolicyRejection::NegationConditionOperator(
                    operator.clone(),
                ));
            }
        }
    }

    Ok(())
}

/// Whether `operator` names the negation of a comparison, which is how the policy language
/// spells one: `StringEquals` compares, `StringNotEquals` negates that comparison. Read
/// without regard to case, as the rest of the document is.
fn negates(operator: &str) -> bool {
    operator.to_ascii_lowercase().contains(&NEGATION_MARKER.to_ascii_lowercase())
}

/// The refusal for a policy document the rule above rejects: the member-constraint
/// `ValidationException` (400) both puts answer with, naming the member and the rule.
pub fn resource_policy_exception(
    rejection: &ResourcePolicyRejection,
) -> crate::ops::types::ValidationException {
    member_constraint_exception(POLICY_MEMBER, &rejection.constraint())
}
