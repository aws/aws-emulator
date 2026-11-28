// SPDX-License-Identifier: CC0-1.0

//! The wording of this service's `ValidationException` message, driven end-to-end over
//! the same `MultiServiceRouter` the emulator boots.
//!
//! Two rules are under test, each asserted over a SET of inputs rather than over one
//! recorded request:
//!
//! 1. **A reported `@pattern` is the expression the WHOLE value must match**: an expression
//!    the model ANCHORS is quoted without the `^` / `$` at its ends, and only those (a `^`
//!    inside a character class (`[^:]+`) and a pattern anchored inside each of its
//!    alternatives are reported verbatim). The rows span five modelled patterns across six
//!    operations and three member names; the `.*` form an UNANCHORED expression is quoted
//!    in is the rule of `tests/all/refused_value_is_quoted_by_binding.rs`.
//! 2. **A violation on an ELEMENT of a collection is reported against the collection
//!    MEMBER** — no `<index>.member` step — with that element shape's WHOLE constraint set
//!    nested inside `Member must satisfy constraint: [ … ]`. The rows span two collection
//!    members over different element shapes, one and several offending elements, an
//!    element that breaks two of its shape's constraints at once, and — as the contrast —
//!    a violation on the collection's OWN `@length`, which is not nested.
//! 3. **A clause quotes the value the member held where the SHAPE reached at that member is
//!    one this service is recorded quoting, and otherwise only where the model binds that
//!    member to the request URI**, so an enum-constrained member in the BODY states
//!    `Value at '<path>' failed to satisfy constraint: Member must satisfy enum value set:
//!    [ … ]` while one in the QUERY string quotes the token it refused. The rows span
//!    enum-constrained members of three operations, in the body and in the query string,
//!    over enums of two, three and five values, and assert the WHOLE clause — the set's
//!    membership AND the order the service lists it in, which
//!    `tests/all/enum_value_set_listing.rs` is the rule for. The binding rule itself is
//!    asserted across every constraint class in
//!    `tests/all/refused_value_is_quoted_by_binding.rs`, and the shape catalogs that decide
//!    it ahead of the binding in `tests/all/refused_subject_follows_the_member_shape.rs`.
//! 4. **The value a clause stated at a COLLECTION member quotes is the value that MEMBER
//!    held** — the whole list the request carried, in position order, comma-separated,
//!    inside one pair of brackets — **and not the elements that offended**, for a collection
//!    whose clause quotes at all
//!    ([`a_collection_clause_quotes_the_whole_list_the_request_carried`]).
//!
//! Every expectation is derived from the model, cited per row
//! (`smithy_query(query="operation")` / `query="shape"`), never from a recorded response
//! body. The requests go through `crate::support` — the ONE harness every suite in this
//! crate drives the emulator with — rather than a second copy of it.

use serde_json::{json, Value};

use crate::support::{self, emulator, label, Emulator};

// ---------------------------------------------------------------------------------------
// The modelled patterns these rows quote, each UNANCHORED — the form a message states.
// ---------------------------------------------------------------------------------------

/// `GuardrailIdentifier`'s `@pattern`, unanchored. Model (`query="shape"`,
/// shape `GuardrailIdentifier`):
/// `^(([a-z0-9]+)|(arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:guardrail/[a-z0-9]+))$`.
/// The `^` inside `(-[^:]+)?` is an ordinary character of a class and stays.
const GUARDRAIL_IDENTIFIER: &str =
    r"(([a-z0-9]+)|(arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:guardrail/[a-z0-9]+))";

/// `GuardrailVersion`'s `@pattern`, unanchored. Model: `^(([1-9][0-9]{0,7})|(DRAFT))$`.
///
/// `pub(crate)` because `tests/all/refused_value_is_quoted_by_binding.rs` quotes the same
/// expression for the same member rather than keeping a second copy of it.
pub(crate) const GUARDRAIL_VERSION: &str = r"(([1-9][0-9]{0,7})|(DRAFT))";

/// `GuardrailNumericalVersion`'s `@pattern`, unanchored. Model: `^[1-9][0-9]{0,7}$`.
const GUARDRAIL_NUMERICAL_VERSION: &str = r"[1-9][0-9]{0,7}";

/// `EvaluationJobIdentifier`'s `@pattern`, unanchored. Model:
/// `^(arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:evaluation-job/[a-z0-9]{12})$`.
///
/// `pub(crate)` because `tests/all/refused_subject_follows_the_member_shape.rs` quotes the
/// same expression for the same shape rather than keeping a second copy of it.
pub(crate) const EVALUATION_JOB_IDENTIFIER: &str =
    r"(arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:evaluation-job/[a-z0-9]{12})";

/// `AdvancedPromptOptimizationJobIdentifier`'s `@pattern`, unanchored. Model:
/// `^((arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:advanced-prompt-optimization-job/)?[a-z0-9]{12})$`.
///
/// `pub(crate)` because `tests/all/refused_subject_follows_the_member_shape.rs` quotes the
/// same expression for the same member rather than keeping a second copy of it.
pub(crate) const APO_JOB_IDENTIFIER: &str = r"((arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:advanced-prompt-optimization-job/)?[a-z0-9]{12})";

/// `AutomatedReasoningPolicyTestCaseId`'s `@pattern`, unanchored. Model (`query="shape"`,
/// shape `AutomatedReasoningPolicyTestCaseId`): `^[0-9A-Z]{12}$`, `@length(max: 12)`.
///
/// `pub(crate)` because `tests/all/refused_subject_follows_the_member_shape.rs` and
/// `tests/all/path_label_values_are_not_echoed.rs` quote the same expression for the same
/// member rather than keeping a second copy of it.
pub(crate) const TEST_CASE_ID: &str = r"[0-9A-Z]{12}";

/// `TaggableResourcesArn`'s `@pattern`, which the model anchors INSIDE each of its
/// alternatives rather than at the ends (it opens `(^[a-zA-Z0-9]…` and closes `…)`), so
/// the reported expression is the model's verbatim: there are no end anchors to strip, and
/// an expression carrying anchors already describes the whole value.
/// Model: `query="shape"`, shape `TaggableResourcesArn`.
///
/// `pub(crate)` for the reason [`GUARDRAIL_VERSION`] is.
pub(crate) const TAGGABLE_RESOURCE_ARN: &str = r"(^[a-zA-Z0-9][a-zA-Z0-9\-]*$)|(^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:custom-model/(imported)/[a-z0-9]{12}$)|(^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:([0-9]{12}|)((:(fine-tuning-job|model-customization-job|custom-model)/[a-z0-9-]{1,63}[.]{1}[a-z0-9-]{1,63}([a-z0-9-]{1,63}[.]){0,2}[a-z0-9-]{1,63}([:][a-z0-9-]{1,63}){0,2}(/[a-z0-9]{12})$)|(:guardrail/[a-z0-9]+$)|(:automated-reasoning-policy/[a-zA-Z0-9]+(:[a-zA-Z0-9]+)?$)|(:(inference-profile|application-inference-profile)/[a-zA-Z0-9-:.]+$)|(:project/(proj_[a-z2-7]{20}|default)$)|(:(provisioned-model|model-invocation-job|model-evaluation-job|evaluation-job|model-import-job|imported-model|async-invoke|provisioned-model-v2|provisioned-model-reservation|prompt-router|custom-model-deployment)/[a-z0-9]{12}$)))";

/// `TagKey`'s `@pattern`, unanchored. Model (`query="shape"`, shape `TagKeyList`): the
/// list's member targets `TagKey`, `@length 1..128`, `@pattern
/// ^[a-zA-Z0-9\s._:/=+@-]*$`.
const TAG_KEY: &str = r"[a-zA-Z0-9\s._:/=+@-]*";

/// The requirement clause a `@pattern` violation states, over the expression as the
/// message quotes it (the one the WHOLE value must match).
///
/// `pub(crate)` for the reason [`GUARDRAIL_VERSION`] is.
pub(crate) fn pattern_requirement(expression: &str) -> String {
    format!("Member must satisfy regular expression pattern: {expression}")
}

/// The subject a clause opens with: the value the request carried, quoted, for a member
/// whose SHAPE this service quotes or that the model binds to the request URI, and the
/// position ALONE for one the service reads out of the request body or whose SHAPE this
/// service states by position alone (`src/service/validation.rs`, rule 2 — the shapes are
/// the rule of `tests/all/refused_subject_follows_the_member_shape.rs`).
fn subject(value: Option<&str>) -> String {
    match value {
        Some(value) => format!("Value '{value}'"),
        None => "Value".to_string(),
    }
}

/// The element constraint set of `AdvancedPromptOptimizationJobIdentifiers`' member:
/// `@length(max: 1011)` — no declared minimum, so the minimum stated is `0` — and the
/// identifier `@pattern` (`query="shape"`,
/// shape `AdvancedPromptOptimizationJobIdentifiers`).
///
/// `pub(crate)` because `tests/all/refused_subject_follows_the_member_shape.rs` states the
/// same set for the same member rather than keeping a second copy of it.
pub(crate) fn apo_element_set() -> String {
    format!(
        "Member must have length less than or equal to 1011, \
         Member must have length greater than or equal to 0, \
         {}",
        pattern_requirement(APO_JOB_IDENTIFIER)
    )
}

/// The element constraint set of `EvaluationJobIdentifiers`' member: the same `@length`
/// and the evaluation-job `@pattern` (`query="shape"`, shape `EvaluationJobIdentifiers`).
///
/// `pub(crate)` because `tests/all/refused_subject_follows_the_member_shape.rs` states the
/// same set for the same member rather than keeping a second copy of it.
pub(crate) fn evaluation_element_set() -> String {
    format!(
        "Member must have length less than or equal to 1011, \
         Member must have length greater than or equal to 0, \
         {}",
        pattern_requirement(EVALUATION_JOB_IDENTIFIER)
    )
}

/// The element constraint set of `TagKeyList`'s member: `@length 1..128` — a DECLARED
/// minimum, so `1` and not `0` — and `TagKey`'s `@pattern`.
fn tag_key_element_set() -> String {
    format!(
        "Member must have length less than or equal to 128, \
         Member must have length greater than or equal to 1, \
         {}",
        pattern_requirement(TAG_KEY)
    )
}

/// One row of a table that drives a request and expects one refusal clause.
///
/// A named struct rather than a tuple: the rows carry a request (method, path, body) and
/// the clause it must produce, and a seven-field tuple reads as nothing at the call site.
struct Row {
    /// What the row varies, named in every assertion.
    case: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
    /// The member path the clause must be reported at.
    member: &'static str,
    /// The requirement the clause must state.
    requirement: String,
    /// The value the clause must echo, when the row pins it.
    value: Option<&'static str>,
}

/// One emulator answer: the status, the wire error code and the `message` the body states.
///
/// A projection of the shared harness's [`crate::support::Reply`], not a second harness:
/// `support::emulator()` is the ONE dispatch path every suite in this crate drives, and
/// this suite only ever reads the refusal's `message` out of the body it returns.
#[derive(Debug)]
struct Refusal {
    status: u16,
    error: Option<String>,
    message: String,
}

impl From<support::Reply> for Refusal {
    fn from(reply: support::Reply) -> Self {
        Self {
            status: reply.status,
            error: reply.error.clone(),
            message: reply
                .get("message")
                .as_str()
                .unwrap_or_default()
                .to_string(),
        }
    }
}

/// Drive one request through the shared harness and read its refusal.
async fn refuse(
    emulator: &Emulator,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Refusal {
    emulator.call(method, path, body).await.into()
}

/// Every `@pattern` this service reports is the expression the whole value must match — for
/// an expression the model anchors, that expression without the `^` / `$` at its ends —
/// over five modelled patterns, six operations and three member names.
///
/// The rejected value differs per row along what the rule generalises over: which pattern
/// the member declares, which operation and member carry it, and whether the model anchors
/// the expression at its ends at all. Whether the clause QUOTES that value is rule 2's
/// question, answered by the shape and then by the member's binding: the URI-bound rows
/// quote it, the body row does not, and neither do the two rows whose shape this service
/// states by position alone.
#[tokio::test]
async fn a_reported_pattern_is_the_expression_the_whole_value_must_match() {
    let emulator = emulator();
    let pattern_row = |case, method, path: String, body, value: Option<&'static str>, member, expression| Row {
        case,
        method,
        path,
        body,
        member,
        requirement: pattern_requirement(expression),
        value,
    };
    let rows = vec![
        pattern_row(
            "GetGuardrail guardrailIdentifier outside GuardrailIdentifier",
            "GET",
            "/guardrails/Not_An_Id".to_string(),
            None,
            Some("Not_An_Id"),
            "guardrailIdentifier",
            GUARDRAIL_IDENTIFIER,
        ),
        pattern_row(
            "GetGuardrail guardrailVersion outside GuardrailVersion",
            "GET",
            "/guardrails/abc123?guardrailVersion=0".to_string(),
            None,
            Some("0"),
            "guardrailVersion",
            GUARDRAIL_VERSION,
        ),
        pattern_row(
            "DeleteGuardrail guardrailVersion outside GuardrailNumericalVersion",
            "DELETE",
            "/guardrails/abc123?guardrailVersion=DRAFT".to_string(),
            None,
            Some("DRAFT"),
            "guardrailVersion",
            GUARDRAIL_NUMERICAL_VERSION,
        ),
        pattern_row(
            "GetEvaluationJob jobIdentifier outside EvaluationJobIdentifier",
            "GET",
            format!(
                "/evaluation-jobs/{}",
                label("arn:aws:bedrock:us-east-1:000000000000:evaluation-job/nonexistent")
            ),
            None,
            Some("arn:aws:bedrock:us-east-1:000000000000:evaluation-job/nonexistent"),
            "jobIdentifier",
            EVALUATION_JOB_IDENTIFIER,
        ),
        pattern_row(
            "StopEvaluationJob jobIdentifier outside EvaluationJobIdentifier",
            "POST",
            format!("/evaluation-job/{}/stop", label("not-a-job")),
            None,
            Some("not-a-job"),
            "jobIdentifier",
            EVALUATION_JOB_IDENTIFIER,
        ),
        pattern_row(
            // The same member NAME over the advanced-prompt-optimization job's identifier
            // shape, which this service states by position alone wherever it is reached
            // (`tests/all/refused_subject_follows_the_member_shape.rs` is the rule).
            "GetAdvancedPromptOptimizationJob jobIdentifier outside its pattern",
            "GET",
            format!(
                "/advanced-prompt-optimization-jobs/{}",
                label("arn:aws:bedrock:us-east-1:000000000000:advanced-prompt-optimization-job/nope")
            ),
            None,
            None,
            "jobIdentifier",
            APO_JOB_IDENTIFIER,
        ),
        pattern_row(
            "StopAdvancedPromptOptimizationJob jobIdentifier outside its pattern",
            "POST",
            format!("/advanced-prompt-optimization-jobs/{}/stop", label("UPPERCASE123")),
            None,
            None,
            "jobIdentifier",
            APO_JOB_IDENTIFIER,
        ),
        pattern_row(
            "UntagResource resourceARN, a BODY member, outside a pattern the model does \
             not anchor at its ends",
            "POST",
            "/untagResource".to_string(),
            Some(json!({ "resourceARN": "not a taggable arn!!", "tagKeys": ["Team"] })),
            // A member the service reads out of the body: the clause states the position
            // alone, so the request's own text is not reflected back (rule 2).
            None,
            "resourceARN",
            TAGGABLE_RESOURCE_ARN,
        ),
    ];

    for row in rows {
        let reply = refuse(&emulator, row.method, &row.path, row.body).await;
        let case = row.case;
        assert_eq!(
            (reply.status, reply.error.as_deref()),
            (400, Some("ValidationException")),
            "case {case}: expected 400 ValidationException, got {} {:?}",
            reply.status,
            reply.message
        );
        let expected = format!(
            "1 validation error detected: {} at '{}' failed to satisfy constraint: {}",
            subject(row.value),
            row.member,
            row.requirement
        );
        assert_eq!(
            reply.message, expected,
            "case {case}: expected {expected:?}, got {:?}",
            reply.message
        );
    }
}

/// Rule 3: an enum clause names the admitted set, and quotes the value the member held
/// only where the model binds that member to the request URI.
///
/// The rows differ along what the rule generalises over: which operation and member carry
/// the enum, whether the value travels in the BODY (reported by position alone) or in the
/// QUERY string (quoted), and how many
/// values the enum publishes (two, three and five). Each row asserts the WHOLE clause: the
/// membership is the model's (`smithy_query(query="shape",shape=...)`) and the order is the
/// one this service lists that enum's values in, which `tests/all/enum_value_set_listing.rs`
/// and `tests/all/enum_value_set_order.rs` are the rule for over a superset of these enums.
#[tokio::test]
async fn an_enum_clause_states_the_admitted_set_and_quotes_a_uri_bound_value() {
    /// One enum-constrained member driven outside its set.
    ///
    /// A named struct rather than a tuple, for the reason [`Row`] is one: the row carries a
    /// request, the member it offends at, the value it offends with and the set the clause
    /// must state.
    struct EnumRow {
        case: &'static str,
        method: &'static str,
        path: &'static str,
        body: Option<Value>,
        member: &'static str,
        /// The value the request carries, and whether the clause quotes it: `Some` for a
        /// member the model binds to the request URI, `None` for one carried in the body.
        quoted: Option<&'static str>,
        /// The values the model admits for that member, in the order this service lists
        /// that enum's values — its own listing for `DataRetentionMode`, the model's
        /// declaration order for an enum it publishes no listing for.
        admitted: &'static [&'static str],
    }

    let emulator = emulator();
    let rows = vec![
        EnumRow {
            // `DataRetentionMode`, five values.
            case: "PutAccountDataRetention mode outside DataRetentionMode, in the body",
            method: "PUT",
            path: "/data-retention",
            body: Some(json!({ "mode": "retain-everything" })),
            member: "mode",
            quoted: None,
            admitted: &[
                "default",
                "provider_data_share",
                "none",
                "aws_review",
                "inherit",
            ],
        },
        EnumRow {
            // `ModelModality`, three values, carried in the query string.
            case: "ListFoundationModels byOutputModality outside ModelModality",
            method: "GET",
            path: "/foundation-models?byOutputModality=AUDIO",
            body: None,
            member: "byOutputModality",
            quoted: Some("AUDIO"),
            admitted: &["TEXT", "IMAGE", "EMBEDDING"],
        },
        EnumRow {
            // `InferenceType`, two values.
            case: "ListFoundationModels byInferenceType outside InferenceType",
            method: "GET",
            path: "/foundation-models?byInferenceType=ONDEMAND",
            body: None,
            member: "byInferenceType",
            quoted: Some("ONDEMAND"),
            admitted: &["ON_DEMAND", "PROVISIONED"],
        },
        EnumRow {
            // `SortOrder`, two values, on another operation.
            case: "ListProvisionedModelThroughputs sortOrder outside SortOrder",
            method: "GET",
            path: "/provisioned-model-throughputs?sortOrder=sideways",
            body: None,
            member: "sortOrder",
            quoted: Some("sideways"),
            admitted: &["Ascending", "Descending"],
        },
    ];

    for row in rows {
        let case = row.case;
        let reply = refuse(&emulator, row.method, row.path, row.body).await;
        assert_eq!(
            (reply.status, reply.error.as_deref()),
            (400, Some("ValidationException")),
            "case {case}: expected 400 ValidationException, got {} {:?}",
            reply.status,
            reply.message
        );
        let expected = format!(
            "1 validation error detected: {} at '{}' failed to satisfy constraint: \
             Member must satisfy enum value set: [{}]",
            subject(row.quoted),
            row.member,
            row.admitted.join(", ")
        );
        assert_eq!(
            reply.message, expected,
            "case {case}: expected {expected:?}, got {:?}",
            reply.message
        );
    }
}

/// A violation on a collection ELEMENT is reported against the collection MEMBER, with the
/// element shape's whole constraint set nested — over two collection members with
/// different element shapes, and lists differing in how many elements offend.
#[tokio::test]
async fn an_element_violation_is_reported_at_its_collection_with_the_elements_constraint_set() {
    let emulator = emulator();
    let evaluation_arn =
        |suffix: &str| format!("arn:aws:bedrock:us-east-1:000000000000:evaluation-job/{suffix}");
    // One row: a request whose only fault is an element of a collection, the member the
    // clause is reported at, the element set it must state, and the value it quotes.
    // Whether the clause quotes the collection is rule 3's question, decided by the ELEMENT
    // shape and then by the member's binding: the evaluation-job identifier is a shape this
    // service quotes wherever it is reached, and the value quoted is the whole list the
    // request carried (rule 4); every other collection here holds an element shape the
    // service states by position alone or one in neither catalog at a body member, so its
    // clause quotes nothing (`tests/all/refused_subject_follows_the_member_shape.rs` is the
    // rule for the catalogs).
    struct ElementRow {
        case: &'static str,
        path: &'static str,
        body: Value,
        member: &'static str,
        element_set: String,
        quoted: Option<String>,
    }
    let rows: Vec<ElementRow> = vec![
        ElementRow {
            case: "BatchDeleteEvaluationJob, one offending element",
            path: "/evaluation-jobs/batch-delete",
            body: json!({ "jobIdentifiers": [evaluation_arn("nonexistent")] }),
            member: "jobIdentifiers",
            element_set: evaluation_element_set(),
            quoted: Some(format!("[{}]", evaluation_arn("nonexistent"))),
        },
        ElementRow {
            case: "BatchDeleteEvaluationJob, two offending elements",
            path: "/evaluation-jobs/batch-delete",
            body: json!({
                "jobIdentifiers": [evaluation_arn("first"), evaluation_arn("second")],
            }),
            member: "jobIdentifiers",
            element_set: evaluation_element_set(),
            quoted: Some(format!(
                "[{}, {}]",
                evaluation_arn("first"),
                evaluation_arn("second")
            )),
        },
        ElementRow {
            case: "BatchDeleteAdvancedPromptOptimizationJob, one offending element",
            path: "/advanced-prompt-optimization-job/batch-delete",
            body: json!({ "jobIdentifiers": ["NOT-AN-ID"] }),
            member: "jobIdentifiers",
            element_set: apo_element_set(),
            quoted: None,
        },
        ElementRow {
            case: "BatchDeleteAdvancedPromptOptimizationJob, three offending elements",
            path: "/advanced-prompt-optimization-job/batch-delete",
            body: json!({ "jobIdentifiers": ["A", "BB", "CCC"] }),
            member: "jobIdentifiers",
            element_set: apo_element_set(),
            quoted: None,
        },
        ElementRow {
            case: "UntagResource, an element below the element shape's minimum length",
            path: "/untagResource",
            body: json!({ "resourceARN": "abcdefghij0123456789", "tagKeys": [""] }),
            member: "tagKeys",
            element_set: tag_key_element_set(),
            quoted: None,
        },
        ElementRow {
            case: "UntagResource, an element outside the element shape's pattern",
            path: "/untagResource",
            body: json!({ "resourceARN": "abcdefghij0123456789", "tagKeys": ["no*stars"] }),
            member: "tagKeys",
            element_set: tag_key_element_set(),
            quoted: None,
        },
    ];

    for row in rows {
        let (case, member) = (row.case, row.member);
        let reply = refuse(&emulator, "POST", row.path, Some(row.body)).await;
        assert_eq!(
            (reply.status, reply.error.as_deref()),
            (400, Some("ValidationException")),
            "case {case}: expected 400 ValidationException, got {} {:?}",
            reply.status,
            reply.message
        );
        let expected = format!(
            "1 validation error detected: {} at '{member}' failed to satisfy \
             constraint: Member must satisfy constraint: [{}]",
            subject(row.quoted.as_deref()),
            row.element_set
        );
        assert_eq!(
            reply.message, expected,
            "case {case}: expected {expected:?}, got {:?}",
            reply.message
        );
        assert!(
            !reply.message.contains(".member"),
            "case {case}: the clause must name the collection member, not an indexed \
             element position, got {:?}",
            reply.message
        );
    }
}

/// Rule 4: the value a clause stated at a COLLECTION quotes is the value that MEMBER held —
/// the whole list the request carried, in position order, inside one pair of brackets — and
/// not the elements that offended.
///
/// Driven over `BatchDeleteEvaluationJob`, whose `jobIdentifiers` list holds a shape this
/// service quotes wherever it is reached (rule 3). The rows differ along what the rule
/// generalises over: how many elements the list carries (one, two, three, and the maximum
/// the model declares, `EvaluationJobIdentifiers` `@length(min: 1, max: 25)`), how many of
/// them offend (one, several, all), WHERE the offender sits (first, middle, last), and the
/// FORM each offending element takes — a full job ARN whose resource id is outside the
/// element shape's character class, a value that is no ARN at all, and one past the element
/// shape's `@length(max: 1011)`.
#[tokio::test]
async fn a_collection_clause_quotes_the_whole_list_the_request_carried() {
    let emulator = emulator();
    // The ARN of an evaluation job whose resource id is `id`: twelve characters of
    // `[a-z0-9]` is inside `EvaluationJobIdentifier`'s `@pattern` and anything else is
    // outside it. No job need exist, since the constraint walk refuses the request before
    // any handler reads state.
    let job = |id: &str| format!("arn:aws:bedrock:us-east-1:000000000000:evaluation-job/{id}");
    // The largest list the model admits, with its LAST element offending: the elements the
    // list is filled with are valid identifiers, so the clause is still one.
    let max_elements = 25;
    let full_batch: Vec<String> = (0..max_elements)
        .map(|position| {
            if position + 1 == max_elements {
                job("LAST")
            } else {
                job(&format!("{:0>12}", format!("job{position}")))
            }
        })
        .collect();
    // (case, the list the request carries, in position order)
    let rows: Vec<(&str, Vec<String>)> = vec![
        (
            "one element, a job ARN outside the element shape's pattern",
            vec![job("NOT-AN-ID")],
        ),
        (
            "one element, a bare identifier that is no ARN at all",
            vec!["not-a-job-identifier".to_string()],
        ),
        (
            "one element, a value past the element shape's maximum length",
            vec!["z".repeat(1012)],
        ),
        (
            "two elements, both offending",
            vec![job("FIRST"), job("SECOND")],
        ),
        (
            "two elements, the second offending: the VALID element is quoted too",
            vec![job("abc123def456"), job("NOPE")],
        ),
        (
            "three elements, the first offending",
            vec![job("UPPER"), job("abc123def456"), job("aaaaaaaaaaaa")],
        ),
        (
            "three elements, the middle one offending",
            vec![
                job("abc123def456"),
                "short".to_string(),
                job("aaaaaaaaaaaa"),
            ],
        ),
        ("the modelled maximum of 25 elements, the last offending", full_batch),
    ];

    for (case, submitted) in rows {
        let reply = refuse(
            &emulator,
            "POST",
            "/evaluation-jobs/batch-delete",
            Some(json!({ "jobIdentifiers": submitted })),
        )
        .await;
        assert_eq!(
            (reply.status, reply.error.as_deref()),
            (400, Some("ValidationException")),
            "case {case}: expected 400 ValidationException, got {} {:?}",
            reply.status,
            reply.message
        );
        let expected = format!(
            "1 validation error detected: {} at 'jobIdentifiers' failed to satisfy \
             constraint: Member must satisfy constraint: [{}]",
            subject(Some(&format!("[{}]", submitted.join(", ")))),
            evaluation_element_set()
        );
        assert_eq!(
            reply.message, expected,
            "case {case}: expected {expected:?}, got {:?}",
            reply.message
        );
        // The two invariants the whole-message assertion rests on, stated on their own so a
        // failure names which one broke: EVERY element the request carried is quoted, valid
        // ones included, and the clause names the collection member rather than an indexed
        // element position.
        for element in &submitted {
            assert!(
                reply.message.contains(element.as_str()),
                "case {case}: the value quoted is the whole list the member held, so every \
                 element must be in it, got {:?}",
                reply.message
            );
        }
        assert!(
            !reply.message.contains(".member"),
            "case {case}: the clause names the collection member, not an indexed element \
             position, got {:?}",
            reply.message
        );
    }
}

/// One element breaking SEVERAL of its shape's constraints is ONE error: the clause states
/// what the member must satisfy, not how many of its rules a value broke.
#[tokio::test]
async fn an_element_breaking_two_constraints_states_one_clause() {
    let emulator = emulator();
    // 1012 characters: one past `@length(max: 1011)`, and outside the identifier
    // `@pattern` as well, so the walk reports both constraints on the one element.
    let oversize = "z".repeat(1012);
    let reply = refuse(
        &emulator,
        "POST",
        "/advanced-prompt-optimization-job/batch-delete",
        Some(json!({ "jobIdentifiers": [oversize] })),
    )
    .await;
    assert_eq!(
        (reply.status, reply.error.as_deref()),
        (400, Some("ValidationException")),
        "case an element past its maximum length: expected 400 ValidationException, got {} {:?}",
        reply.status,
        reply.message
    );
    assert!(
        reply
            .message
            .starts_with("1 validation error detected: Value at 'jobIdentifiers'"),
        "case an element past its maximum length: two broken constraints of one element \
         shape are one error, got {:?}",
        reply.message
    );
    let expected_tail = format!(
        "at 'jobIdentifiers' failed to satisfy constraint: Member must satisfy constraint: \
         [{}]",
        apo_element_set()
    );
    assert!(
        reply.message.ends_with(&expected_tail),
        "case an element past its maximum length: expected the clause to end {expected_tail:?}, \
         got {:?}",
        reply.message
    );
    assert_eq!(
        reply.message.matches("Member must satisfy constraint: [").count(),
        1,
        "case an element past its maximum length: the element set is stated once, got {:?}",
        reply.message
    );
}

/// A collection only PARTLY invalid still names the member and the element shape's whole
/// constraint set.
///
/// Nothing of the collection is quoted: its element shape is one this service states by
/// position alone wherever it is reached, so the clause states the member's position and
/// what every element of it must satisfy (rule 3). The contrast is
/// [`a_collection_clause_quotes_the_whole_list_the_request_carried`], where a partly invalid
/// list of the QUOTED shape is echoed whole.
#[tokio::test]
async fn a_partly_invalid_collection_names_the_member_and_its_element_constraint_set() {
    let emulator = emulator();
    // `[a-z0-9]{12}` is inside `AdvancedPromptOptimizationJobIdentifier`'s pattern, so the
    // first element is accepted and only the second offends.
    let reply = refuse(
        &emulator,
        "POST",
        "/advanced-prompt-optimization-job/batch-delete",
        Some(json!({ "jobIdentifiers": ["abc123def456", "NOT-AN-ID"] })),
    )
    .await;
    assert_eq!(
        (reply.status, reply.error.as_deref()),
        (400, Some("ValidationException")),
        "case one valid and one invalid element: expected 400 ValidationException, got {} {:?}",
        reply.status,
        reply.message
    );
    let expected = format!(
        "1 validation error detected: Value at 'jobIdentifiers' failed to \
         satisfy constraint: Member must satisfy constraint: [{}]",
        apo_element_set()
    );
    assert_eq!(
        reply.message, expected,
        "case one valid and one invalid element: expected {expected:?}, got {:?}",
        reply.message
    );
}

/// The contrast rows: a violation on a member ITSELF is NOT nested in the element-set
/// clause, whatever the member's type — the collection's own `@length`, a scalar's
/// `@length`, a `@range` and an absent `@required` member alike.
#[tokio::test]
async fn a_violation_on_the_member_itself_is_not_nested() {
    let emulator = emulator();
    let valid_guardrail = |extra: Value| {
        let mut body = json!({
            "name": "valid",
            "blockedInputMessaging": "in",
            "blockedOutputsMessaging": "out",
        });
        for (key, value) in extra.as_object().expect("an object") {
            body[key] = value.clone();
        }
        body
    };
    let rows = vec![
        Row {
            // `EvaluationJobIdentifiers` is `@length(min: 1, max: 25)`: the EMPTY list
            // breaks the collection's own minimum, not any element's constraint.
            case: "BatchDeleteEvaluationJob with an empty jobIdentifiers list",
            method: "POST",
            path: "/evaluation-jobs/batch-delete".to_string(),
            body: Some(json!({ "jobIdentifiers": [] })),
            member: "jobIdentifiers",
            requirement: "Member must have length greater than or equal to 1".to_string(),
            value: None,
        },
        Row {
            // `GuardrailName` is `@length(min: 1, max: 50)`.
            case: "CreateGuardrail with a name one past its maximum length",
            method: "POST",
            path: "/guardrails".to_string(),
            body: Some(valid_guardrail(json!({ "name": "a".repeat(51) }))),
            member: "name",
            requirement: "Member must have length less than or equal to 50".to_string(),
            value: None,
        },
        Row {
            // `ListGuardrails$maxResults` is `@range(min: 1, max: 1000)`.
            case: "ListGuardrails with maxResults below its range",
            method: "GET",
            path: "/guardrails?maxResults=0".to_string(),
            body: None,
            member: "maxResults",
            requirement: "Member must have value greater than or equal to 1".to_string(),
            value: None,
        },
        Row {
            case: "CreateGuardrail without its required name",
            method: "POST",
            path: "/guardrails".to_string(),
            body: Some(json!({
                "blockedInputMessaging": "in",
                "blockedOutputsMessaging": "out",
            })),
            member: "name",
            requirement: "Member must not be null".to_string(),
            value: None,
        },
    ];

    for row in rows {
        let reply = refuse(&emulator, row.method, &row.path, row.body).await;
        let case = row.case;
        assert_eq!(
            (reply.status, reply.error.as_deref()),
            (400, Some("ValidationException")),
            "case {case}: expected 400 ValidationException, got {} {:?}",
            reply.status,
            reply.message
        );
        let expected_tail = format!(
            "at '{}' failed to satisfy constraint: {}",
            row.member, row.requirement
        );
        assert!(
            reply.message.ends_with(&expected_tail),
            "case {case}: expected the clause to end {expected_tail:?}, got {:?}",
            reply.message
        );
        assert!(
            !reply.message.contains("Member must satisfy constraint: ["),
            "case {case}: a violation on the member itself must not be nested in the \
             element-set clause, got {:?}",
            reply.message
        );
    }
}

/// Two members of one request each state their own clause, and the envelope counts them.
#[tokio::test]
async fn two_offending_members_state_two_clauses() {
    let emulator = emulator();
    let reply = refuse(
        &emulator,
        "POST",
        "/untagResource",
        Some(json!({ "resourceARN": "short", "tagKeys": ["no*stars"] })),
    )
    .await;
    assert_eq!(
        (reply.status, reply.error.as_deref()),
        (400, Some("ValidationException")),
        "case a bad resourceARN beside a bad tag key: expected 400 ValidationException, got {} {:?}",
        reply.status,
        reply.message
    );
    // `TaggableResourcesArn` is `@length(min: 20, max: 1011)`, and Smithy's `@pattern` is
    // a FIND, so `short` breaks the minimum length while still matching the ARN pattern's
    // first alternative; the tag key breaks its element shape's pattern. Two members, two
    // clauses.
    assert!(
        reply.message.starts_with("2 validation errors detected: "),
        "case a bad resourceARN beside a bad tag key: the envelope counts every distinct \
         clause, got {:?}",
        reply.message
    );
    for fragment in [
        "at 'resourceARN' failed to satisfy constraint: Member must have length greater than \
         or equal to 20",
        &format!(
            "at 'tagKeys' failed to satisfy constraint: Member must satisfy constraint: [{}]",
            tag_key_element_set()
        ),
    ] {
        assert!(
            reply.message.contains(fragment),
            "case a bad resourceARN beside a bad tag key: expected the envelope to state \
             {fragment:?}, got {:?}",
            reply.message
        );
    }
}
