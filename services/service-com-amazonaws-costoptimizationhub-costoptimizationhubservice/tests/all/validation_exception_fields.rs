// SPDX-License-Identifier: CC0-1.0

//! The structured `ValidationException` this service answers a refused request
//! with, driven end-to-end over the wire (`awsJson1_0`) through the same
//! multi-service router the emulator boots.
//!
//! The rule under test: a request Cost Optimization Hub refuses because a member
//! is outside a constraint that member must satisfy — one its model declares, or
//! the value set the service accepts on the wire where that is the wider of the
//! two (`status` below) — is answered with HTTP 400 `ValidationException`
//! carrying `reason` = `FieldValidationFailed` and one `fields` entry per
//! violation, each naming the member it is attributed to and the constraint that
//! member must satisfy
//! (`smithy_query(query="shape", shape="ValidationException,ValidationExceptionDetail,ValidationExceptionReason")`).
//!
//! It is asserted over a table that spans the constraint classes the model uses
//! (`@pattern`, `@range`, `@length`, enum membership, `@required`), the positions
//! a violation can hold (a scalar member, a member of a nested structure, an
//! element of a nested list), the number of violations in one request, and the
//! operations that declare them.

use serde_json::{json, Value};
use service_com_amazonaws_costoptimizationhub_costoptimizationhubservice as cost_optimization_hub;

use crate::cost_optimization_hub::{
    call, compute_optimizer_opted_in_router, error_code, monthly_period, router,
    FIELD_VALIDATION_FAILED,
};

/// The model's enum value sets, as the generated catalogue holds them — the same
/// model data the constraint walk checks against, so the clause a refusal states
/// is compared with the model rather than with a list copied into this file.
use cost_optimization_hub::ops::types::shared_types::{
    GRANULARITY_TYPE_VALUES, IMPLEMENTATION_EFFORT_VALUES,
    MEMBER_ACCOUNT_DISCOUNT_VISIBILITY_VALUES, SAVINGS_ESTIMATION_MODE_VALUES,
};

/// The `EnrollmentStatus` values the SERVICE accepts on the wire, which is what
/// `UpdateEnrollmentStatus` judges its `status` against — wider than the
/// published model's enum, so this set is read from the service rather than from
/// the generated catalogue (`crate::rejection_fidelity` owns that rule).
use cost_optimization_hub::state::ENROLLMENT_STATUS_WIRE_VALUES;

/// The `fields` list of a `ValidationException` body as `(fieldName, message)`
/// pairs, SORTED so a case does not depend on the order the walk found the
/// violations in — which is the only thing this suite needs beyond the shared
/// extractor.
fn fields(payload: &Value) -> Vec<(String, String)> {
    let mut entries = crate::cost_optimization_hub::fields(payload);
    entries.sort();
    entries
}

/// The clause a violation found on an ELEMENT of the named member states: the
/// element's own requirement, bracketed, because the member reported is the one
/// that holds the element.
fn element_clause(requirement: &str) -> String {
    format!("Member must satisfy constraint: [{requirement}]")
}

/// `AccountId`'s requirement, stated without the anchors the model spells it with
/// (`smithy_query(query="shape", shape="AccountId")`: `@pattern ^[0-9]{12}$`).
const ACCOUNT_ID_CLAUSE: &str = "Member must satisfy regular expression pattern: [0-9]{12}";

/// What one `fields` entry must say.
#[derive(Debug)]
enum Clause {
    /// Exactly this requirement.
    Exact(String),
    /// Enum membership over the model's value set, held at the named member
    /// itself (`on_element` false) or at an element of it (`on_element` true).
    ///
    /// The set is compared as a SET: which values the clause lists is the
    /// model's, but the order it lists them in is the model's declaration order
    /// and not a rule of the refusal.
    EnumSet {
        values: &'static [&'static str],
        on_element: bool,
    },
}

impl Clause {
    /// Whether `actual` is the clause this expectation describes.
    fn states(&self, actual: &str) -> bool {
        match self {
            Self::Exact(expected) => actual == expected,
            Self::EnumSet { values, on_element } => {
                let inner = if *on_element {
                    actual
                        .strip_prefix("Member must satisfy constraint: [")
                        .and_then(|inner| inner.strip_suffix(']'))
                } else {
                    Some(actual)
                };
                let listed = inner
                    .and_then(|inner| inner.strip_prefix("Member must satisfy enum value set: ["))
                    .and_then(|listed| listed.strip_suffix(']'));
                let Some(listed) = listed else {
                    return false;
                };
                let mut got: Vec<&str> = listed.split(", ").collect();
                got.sort_unstable();
                let mut wanted: Vec<&str> = values.to_vec();
                wanted.sort_unstable();
                got == wanted
            }
        }
    }
}

/// One `fields` entry, as a case writes its expectation: an exact requirement.
fn entry(field_name: &str, requirement: impl Into<String>) -> (&str, Clause) {
    (field_name, Clause::Exact(requirement.into()))
}

/// An `accountIds` filter one past its published maximum length
/// (`AccountIdList @length(min: 1, max: 100)`), whose last element is also
/// outside `AccountId`'s pattern — so the one member fails two distinct rules.
fn over_long_account_ids() -> Vec<String> {
    let mut ids: Vec<String> = (0..100).map(|n| format!("{n:012}")).collect();
    ids.push("12345".to_string());
    ids
}

/// One `fields` entry stating enum membership at the named member.
fn enum_entry<'a>(field_name: &'a str, values: &'static [&'static str]) -> (&'a str, Clause) {
    (
        field_name,
        Clause::EnumSet {
            values,
            on_element: false,
        },
    )
}

/// One `fields` entry stating enum membership at an ELEMENT of the named member.
fn enum_element_entry<'a>(
    field_name: &'a str,
    values: &'static [&'static str],
) -> (&'a str, Clause) {
    (
        field_name,
        Clause::EnumSet {
            values,
            on_element: true,
        },
    )
}

/// One row of the refusal table: the case name, the operation called, the request
/// sent, and the `fields` entries the answer must carry.
type RefusalCase<'a> = (&'a str, &'a str, Value, Vec<(&'a str, Clause)>);

/// Every refused request is 400 `ValidationException` with
/// `reason = FieldValidationFailed` and one `fields` entry per violation, naming
/// the member it is attributed to and what that member must satisfy.
#[tokio::test]
async fn a_constraint_refusal_names_every_member_and_its_constraint() {
    let router = router();

    // (case, operation, request, the fields entries the answer must carry)
    let cases: Vec<RefusalCase> = vec![
        (
            "pattern on a scalar member",
            "ListEnrollmentStatuses",
            json!({"includeOrganizationInfo": false, "accountId": "12345"}),
            vec![entry("accountId", ACCOUNT_ID_CLAUSE)],
        ),
        (
            // Same class, a value the issue did not cite: too LONG rather than
            // too short, and with the defaulted member omitted.
            "pattern on a scalar member, thirteen digits",
            "ListEnrollmentStatuses",
            json!({"accountId": "9876543210987"}),
            vec![entry("accountId", ACCOUNT_ID_CLAUSE)],
        ),
        (
            // `MaxResults` is `@range(min: 0, max: 100)`, so each published bound
            // is accepted (see `every_request_inside_its_constraints_is_served`)
            // and one past it is refused. This operation's own `maxResults`
            // member declares the same bound, so the shape's is the one applied;
            // the three operations whose member widens it to 0..1000 are
            // asserted in `crate::rejection_fidelity`.
            "range above the published maximum",
            "ListEnrollmentStatuses",
            json!({"includeOrganizationInfo": false, "maxResults": 101}),
            vec![entry(
                "maxResults",
                "Member must have value less than or equal to 100",
            )],
        ),
        (
            "range below the published minimum",
            "ListEnrollmentStatuses",
            json!({"includeOrganizationInfo": false, "maxResults": -1}),
            vec![entry(
                "maxResults",
                "Member must have value greater than or equal to 0",
            )],
        ),
        (
            // The accepted set is the service's wire set, not the published
            // model's enum: the member's own validation is the handler's.
            "enum membership on a required member",
            "UpdateEnrollmentStatus",
            json!({"status": "Enabled"}),
            vec![enum_entry("status", ENROLLMENT_STATUS_WIRE_VALUES)],
        ),
        (
            "a required member absent altogether",
            "UpdateEnrollmentStatus",
            json!({}),
            vec![entry("status", "Member must not be null")],
        ),
        (
            "a required member of an implemented operation absent",
            "GetRecommendation",
            json!({}),
            vec![entry("recommendationId", "Member must not be null")],
        ),
        (
            // The violation is on an ELEMENT of a list nested in a structure, so
            // it is attributed to the list member that holds it.
            "pattern on an element of a nested list",
            "ListRecommendations",
            json!({"filter": {"accountIds": ["123456789012", "12345"]}}),
            vec![entry(
                "accountIds",
                element_clause(ACCOUNT_ID_CLAUSE),
            )],
        ),
        (
            "enum membership on an element of a nested list",
            "ListRecommendations",
            json!({"filter": {"implementationEfforts": ["Gigantic"]}}),
            vec![enum_element_entry(
                "implementationEfforts",
                IMPLEMENTATION_EFFORT_VALUES,
            )],
        ),
        (
            // The length of the nested list itself, not of an element: attributed
            // to the same member, stated unbracketed.
            "length of a nested list",
            "ListRecommendations",
            json!({"filter": {"accountIds": []}}),
            vec![entry(
                "accountIds",
                "Member must have length greater than or equal to 1",
            )],
        ),
        (
            // `fields` is the list of INVALID FIELDS, so a member whose values
            // failed the same rule twice is named once and states that rule once.
            "two elements of one nested list",
            "ListRecommendations",
            json!({"filter": {"accountIds": ["12345", "abc"]}}),
            vec![entry("accountIds", element_clause(ACCOUNT_ID_CLAUSE))],
        ),
        (
            // One member, two DISTINCT requirements — its elements' pattern and
            // its own length — stated as the bracketed list of both, in the order
            // the walk reached them.
            "a nested list that fails its own bound and its elements'",
            "ListRecommendations",
            json!({"filter": {"accountIds": over_long_account_ids()}}),
            vec![entry(
                "accountIds",
                element_clause(&format!(
                    "{ACCOUNT_ID_CLAUSE}, Member must have length less than or equal to 100"
                )),
            )],
        ),
        (
            "two members at once",
            "ListEnrollmentStatuses",
            json!({"includeOrganizationInfo": false, "accountId": "12345", "maxResults": 101}),
            vec![
                entry("accountId", ACCOUNT_ID_CLAUSE),
                entry(
                    "maxResults",
                    "Member must have value less than or equal to 100",
                ),
            ],
        ),
        (
            // The length of a list member, on an operation whose handler reads
            // the member: the refusal is the model's, raised before the handler.
            "length on a list member of an implemented operation",
            "ListRecommendationSummaries",
            json!({"groupBy": "ActionType", "metrics": []}),
            vec![entry(
                "metrics",
                "Member must have length greater than or equal to 1",
            )],
        ),
        (
            "enum membership on an operation's enum member",
            "ListEfficiencyMetrics",
            json!({
                "granularity": "Hourly",
                "timePeriod": {"start": "2026-01-01", "end": "2026-02-01"},
            }),
            vec![enum_entry("granularity", GRANULARITY_TYPE_VALUES)],
        ),
        (
            "enum membership on a write",
            "UpdatePreferences",
            json!({"savingsEstimationMode": "Nope"}),
            vec![enum_entry(
                "savingsEstimationMode",
                SAVINGS_ESTIMATION_MODE_VALUES,
            )],
        ),
        (
            // The organization-scoped member is judged exactly like the others:
            // a value outside its enum is answered with the field list. A value
            // inside it is then judged against the rule about the CALLER
            // (`preference_gates`), which answers a different message.
            "enum membership on the member-account visibility member",
            "UpdatePreferences",
            json!({"memberAccountDiscountVisibility": "Everyone"}),
            vec![enum_entry(
                "memberAccountDiscountVisibility",
                MEMBER_ACCOUNT_DISCOUNT_VISIBILITY_VALUES,
            )],
        ),
        (
            "two members at once on a write",
            "UpdatePreferences",
            json!({"memberAccountDiscountVisibility": "Everyone", "savingsEstimationMode": "Nope"}),
            vec![
                enum_entry(
                    "memberAccountDiscountVisibility",
                    MEMBER_ACCOUNT_DISCOUNT_VISIBILITY_VALUES,
                ),
                enum_entry("savingsEstimationMode", SAVINGS_ESTIMATION_MODE_VALUES),
            ],
        ),
    ];

    for (case, operation, request, expected) in &cases {
        let (status, payload) = call(&router, operation, request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {payload}"
        );
        let code = error_code(&payload);
        assert_eq!(
            code, "ValidationException",
            "case {case:?}: expected a ValidationException, got {code:?} in {payload}"
        );
        let reason = payload["reason"].as_str().unwrap_or_default();
        assert_eq!(
            reason, FIELD_VALIDATION_FAILED,
            "case {case:?}: expected reason {FIELD_VALIDATION_FAILED:?}, got {reason:?} in {payload}"
        );
        let mut wanted: Vec<&(&str, Clause)> = expected.iter().collect();
        wanted.sort_by_key(|(field_name, _)| *field_name);
        let actual = fields(&payload);
        assert_eq!(
            actual.len(),
            wanted.len(),
            "case {case:?}: expected {} field entries ({wanted:?}), got {actual:?} in {payload}",
            wanted.len()
        );
        for ((field_name, message), (wanted_name, wanted_clause)) in actual.iter().zip(&wanted) {
            assert_eq!(
                field_name, wanted_name,
                "case {case:?}: expected a field entry for {wanted_name:?}, \
                 got {field_name:?} in {payload}"
            );
            assert!(
                wanted_clause.states(message),
                "case {case:?}: field {field_name:?}: expected {wanted_clause:?}, \
                 got {message:?}"
            );
        }
        // The required `message` is still there: a summary, with the per-member
        // diagnosis in `fields`.
        assert!(
            payload["message"]
                .as_str()
                .is_some_and(|message| !message.is_empty()),
            "case {case:?}: expected a non-empty message, got {payload}"
        );
    }
}

/// A refusal a handler raises for a rule the model cannot express fills the same
/// two members, so a client reads one shape whichever layer refused the request.
#[tokio::test]
async fn a_handler_raised_refusal_names_its_member_too() {
    let router = router();

    // (case, operation, request, the member the refusal is attributed to)
    let cases: [(&str, &str, Value, &str); 6] = [
        (
            "a groupBy that names no dimension",
            "ListRecommendationSummaries",
            json!({"groupBy": "NotADimension"}),
            "groupBy",
        ),
        (
            "a groupBy naming a per-resource identifier",
            "ListRecommendationSummaries",
            json!({"groupBy": "ResourceId"}),
            "groupBy",
        ),
        (
            "a filter this operation does not support",
            "ListRecommendationSummaries",
            json!({"groupBy": "ResourceType", "filter": {"resourceIds": ["i-0a1b2c3d4e5f60001"]}}),
            "filter.resourceIds",
        ),
        (
            "a nextToken this service never issued",
            "ListEnrollmentStatuses",
            json!({"includeOrganizationInfo": false, "nextToken": "not-a-token"}),
            "nextToken",
        ),
        (
            "a nextToken from another list",
            "ListRecommendations",
            json!({"nextToken": "6e6f7065"}),
            "nextToken",
        ),
        (
            "an orderBy dimension that names no member",
            "ListRecommendations",
            json!({"orderBy": {"dimension": "NotADimension", "order": "Asc"}}),
            "orderBy.dimension",
        ),
    ];

    for (case, operation, request, member) in &cases {
        let (status, payload) = call(&router, operation, request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {payload}"
        );
        let reason = payload["reason"].as_str().unwrap_or_default();
        assert_eq!(
            reason, FIELD_VALIDATION_FAILED,
            "case {case:?}: expected reason {FIELD_VALIDATION_FAILED:?}, got {reason:?} in {payload}"
        );
        let named: Vec<String> = fields(&payload)
            .into_iter()
            .map(|(field_name, _)| field_name)
            .collect();
        assert_eq!(
            named,
            vec![(*member).to_string()],
            "case {case:?}: expected fields naming {member:?}, got {named:?} in {payload}"
        );
    }
}

/// A request inside every constraint it declares is served, not refused: each
/// published bound of a `@range`, every value of an enum, a list at its minimum
/// length, and a 12-digit account id — the refusal layer added for the field
/// list must not turn a valid request into a 400.
///
/// The account is opted into AWS Compute Optimizer, which the efficiency-metrics
/// read below needs before it is served at all: this suite is about the
/// constraint layer, so the precondition that read states is established rather
/// than asserted here (`efficiency_metrics_compute_optimizer_gate` owns it).
#[tokio::test]
async fn every_request_inside_its_constraints_is_served() {
    let router = compute_optimizer_opted_in_router().await;

    // (case, operation, request, the status the request must get)
    let mut cases: Vec<(String, &str, Value, u16)> = vec![
        (
            "maxResults at the published minimum".to_string(),
            "ListEnrollmentStatuses",
            json!({"includeOrganizationInfo": false, "maxResults": 0}),
            200,
        ),
        (
            "maxResults at the published maximum".to_string(),
            "ListEnrollmentStatuses",
            json!({"includeOrganizationInfo": false, "maxResults": 100}),
            200,
        ),
        (
            "a twelve-digit accountId".to_string(),
            "ListEnrollmentStatuses",
            json!({"includeOrganizationInfo": false, "accountId": engine::emulator_account_id()}),
            200,
        ),
        (
            "a nested list at its minimum length".to_string(),
            "ListRecommendations",
            json!({"filter": {"regions": ["us-east-1"]}}),
            200,
        ),
        (
            "an enum value on a nested list element".to_string(),
            "ListRecommendations",
            json!({"filter": {"implementationEfforts": ["VeryLow"]}}),
            200,
        ),
        (
            "a valid request to a time-series read".to_string(),
            "ListEfficiencyMetrics",
            {
                // The period comes from the clock: that read reports over a
                // window ending at the present interval.
                let (start, end) = monthly_period(1, 1);
                json!({
                    "granularity": "Monthly",
                    "timePeriod": {"start": start, "end": end},
                })
            },
            200,
        ),
        (
            "a valid request to an implemented list operation".to_string(),
            "ListRecommendationSummaries",
            json!({"groupBy": "ActionType", "metrics": ["SavingsPercentage"]}),
            200,
        ),
    ];
    // Every value the service's `EnrollmentStatus` wire set holds is accepted, so
    // the refusal above is about membership and not about the member.
    for status in ENROLLMENT_STATUS_WIRE_VALUES {
        cases.push((
            format!("the enrollment status {status:?}"),
            "UpdateEnrollmentStatus",
            json!({"status": status}),
            200,
        ));
    }

    for (case, operation, request, expected) in &cases {
        let (status, payload) = call(&router, operation, request).await;
        assert_eq!(
            status, *expected,
            "case {case:?}: expected HTTP {expected}, got {status} with {payload}"
        );
        assert!(
            payload["fields"].is_null(),
            "case {case:?}: a request inside its constraints carries no field list, got {payload}"
        );
    }
}
