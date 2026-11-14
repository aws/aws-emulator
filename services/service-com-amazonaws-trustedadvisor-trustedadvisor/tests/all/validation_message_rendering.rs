// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS a request it refuses for breaking a modelled input
//! constraint, over HTTP through the same `MultiServiceRouter` the emulator serves with.
//!
//! A `ValidationException` message is BUILT FROM THE REQUEST — `<N> validation error(s)
//! detected: ` over one `<subject> at '<member path>' failed to satisfy constraint:
//! <requirement>` clause per failing member — so the sentence is part of the contract and
//! not prose. Each test below pins one RULE of that sentence over a TABLE whose rows span
//! the class the rule covers (every member the rule reaches, in query, path-label and
//! body positions, at several values, sizes and element counts), rather than the one call
//! a report happened to record:
//!
//! 1. a reported `@pattern` is the expression a value must satisfy, WITHOUT the `^`/`$`
//!    anchors the model writes to demand a whole-string match;
//! 2. an enum's value set is listed in the order this service reports that enum in, which
//!    is a property of the enum SHAPE — the same for every member targeting it — and an
//!    enum with no recorded order keeps the one the model declares;
//! 3. a member whose target shape the model marks `@sensitive` has its constraint stated
//!    WITHOUT its value, while every other member echoes the value the caller submitted;
//! 4. a clause about a COLLECTION member states the collection the caller sent, each
//!    structure element rendered `<Shape>(<member>=<value>, …)` inside `[…]`, rather than
//!    a count of its elements;
//! 5. the envelope counts the clauses and joins them with `; `.
//!
//! Every status and error code asserted here comes from the model:
//! `smithy_query(query="shape",shape="ValidationException",
//! model_path="services/service-com-amazonaws-trustedadvisor-trustedadvisor/smithy-model/com-amazonaws-trustedadvisor-trustedadvisor.json")`
//! — `@error("client")`, `@httpError(400)`, one `@required` member `message` — and every
//! constraint (pattern, length, range, enum value set, list bounds) comes from
//! `smithy_query(query="operation", …)` over the 12 operations the service binds.

use serde_json::{json, Value};
use service_com_amazonaws_trustedadvisor_trustedadvisor::state::SupportPlan;
use smithy_server_runtime::MultiServiceRouter;

use crate::recommendation_loop::{
    dispatch, emulator_on_plan, error_code, label, recommendation_arn, resource_arn,
};

/// The modelled `@pattern` of `AccountRecommendationIdentifier` as a clause reports it:
/// the expression without its anchors. Source
/// (`smithy_query(query="shape",shape="AccountRecommendationIdentifier")`):
/// `^arn:[\w-]+:trustedadvisor::\d{12}:recommendation\/[\w-]+$`, `@length(20, 200)`.
const RECOMMENDATION_IDENTIFIER_PATTERN: &str =
    r"arn:[\w-]+:trustedadvisor::\d{12}:recommendation\/[\w-]+";

/// `OrganizationRecommendationIdentifier`'s pattern, unanchored. Source:
/// `^arn:[\w-]+:trustedadvisor:::organization-recommendation\/[\w-]+$`, `@length(20, 200)`.
const ORGANIZATION_IDENTIFIER_PATTERN: &str =
    r"arn:[\w-]+:trustedadvisor:::organization-recommendation\/[\w-]+";

/// `AwsResourceArn`'s pattern, unanchored. Source:
/// `^arn:aws(-\w+)*:[\w\d-]+:([\w\d-]*)?:[\w\d_-]*([:/].+)*$`, `@length(20, 2048)`.
pub(crate) const AWS_RESOURCE_ARN_PATTERN: &str =
    r"arn:aws(-\w+)*:[\w\d-]+:([\w\d-]*)?:[\w\d_-]*([:/].+)*";

/// The pattern `CheckIdentifier` and `CheckArn` share, unanchored. Source:
/// `^arn:[\w-]+:trustedadvisor:::check\/[\w-]+$` on both.
pub(crate) const CHECK_PATTERN: &str = r"arn:[\w-]+:trustedadvisor:::check\/[\w-]+";

/// `AccountId`'s pattern, unanchored. Source: `^\d+$`, `@length(12, 12)`.
const ACCOUNT_ID_PATTERN: &str = r"\d+";

/// `RecommendationPillar`'s value set as this service reports it — a recorded AWS answer
/// (see the issue this suite was written for); the SET itself is the model's.
const REPORTED_PILLAR: &str = "[security, service_limits, cost_optimizing, \
                               operational_excellence, fault_tolerance, performance]";

/// `RecommendationSource`'s value set as this service reports it, same source.
const REPORTED_SOURCE: &str = "[rds, pse, well_architected, aws_config, manual, \
                               resilience_hub, stir, compute_optimizer, resilience, \
                               cost_explorer, cost_optimization_hub, ta_check, \
                               security_hub, lse]";

/// `RecommendationType`'s value set as this service reports it, same source.
const REPORTED_TYPE: &str = "[priority, standard]";

/// The value set `RecommendationStatus` and `ResourceStatus` are both reported with: they
/// declare the same values in the same model order, so no recorded answer tells them apart.
const REPORTED_STATUS: &str = "[error, warning, ok]";

/// An `OrganizationRecommendationIdentifier` the modelled pattern admits, so a row about
/// another member of the same request is refused for that member alone.
const ORGANIZATION_RECOMMENDATION: &str =
    "arn:aws:trustedadvisor:::organization-recommendation/00000000-0000-0000-0000-000000000000";

/// One row of a constraint-refusal table: what the row is about, the request to make
/// (method, path and query, body) and the clause it must be refused with — the dotted
/// member path the clause names, the value the request sent for it, and the tail of the
/// requirement that member broke (the reported pattern, or the reported enum value set).
type ConstraintRow = (
    &'static str,
    &'static str,
    String,
    Option<Value>,
    &'static str,
    &'static str,
    &'static str,
);

/// One exclusion entry a batch carries: the resource ARN and the exclusion status.
type ExclusionEntry = (String, bool);

/// One row of the exclusion-batch table: what the row is about, the entries it sends, and
/// the requirement the list broke.
type BatchRow = (String, Vec<ExclusionEntry>, String);

/// An emulator whose account's support plan includes the Trusted Advisor API, so no row
/// here is answered by the support-level gate instead. (Shape validation precedes that
/// gate either way — `tests/all/support_level_gate.rs` pins the order — but a row must
/// fail for the reason it is about.)
fn emulator() -> MultiServiceRouter {
    emulator_on_plan(SupportPlan::Business).0
}

/// The `message` an error body carries, or the empty string when it carries none.
pub(crate) fn message_of(body: &Value) -> &str {
    body["message"].as_str().unwrap_or_default()
}

/// The envelope one clause goes into.
pub(crate) fn one_error(clause: &str) -> String {
    format!("1 validation error detected: {clause}")
}

/// The whole message a `@pattern` violation on `member` is refused with.
fn pattern_message(member: &str, value: &str, pattern: &str) -> String {
    one_error(&format!(
        "Value '{value}' at '{member}' failed to satisfy constraint: Member must satisfy \
         regular expression pattern: {pattern}"
    ))
}

/// The whole message an enum violation on `member` is refused with.
fn enum_message(member: &str, value: &str, values: &str) -> String {
    one_error(&format!(
        "Value '{value}' at '{member}' failed to satisfy constraint: Member must satisfy \
         enum value set: {values}"
    ))
}

/// The whole message a violation on the one `@sensitive` member is refused with: the
/// requirement, and no value.
fn sensitive_message(requirement: &str) -> String {
    one_error(&format!(
        "Value at 'updateReason' failed to satisfy constraint: {requirement}"
    ))
}

/// Assert that one request is refused as a `ValidationException` whose message is exactly
/// `expected`: the status the model binds the shape to, the shape name in BOTH places
/// restJson1 carries it, and the sentence.
pub(crate) async fn assert_refused(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    path_and_query: &str,
    body: Option<Value>,
    expected: &str,
) {
    let (status, errortype, response) = dispatch(router, method, path_and_query, body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {response}"
    );
    assert_eq!(
        error_code(&response),
        "ValidationException",
        "case {case:?}: expected __type ValidationException, got {response}"
    );
    assert_eq!(
        errortype.as_deref(),
        Some("ValidationException"),
        "case {case:?}: expected x-amzn-errortype ValidationException, got {errortype:?}"
    );
    assert_eq!(
        message_of(&response),
        expected,
        "case {case:?}: expected the message {expected:?}, got {:?}",
        message_of(&response)
    );
}

// ---------------------------------------------------------------------------
// Rule 1: a reported `@pattern` carries no anchors
// ---------------------------------------------------------------------------

/// Every member of this service whose model declares a `@pattern`, each refused with a
/// value of a legal LENGTH (so the pattern is the only rule broken). The rows differ
/// along the member, the operation, the pattern itself and where the value rides — a URI
/// path label or a query parameter.
#[tokio::test]
async fn a_pattern_violation_reports_the_expression_without_its_anchors() {
    let router = emulator();
    let organization = label(ORGANIZATION_RECOMMENDATION);
    let resource = label("arn:aws:ec2:us-east-1:000000000000:instance/i-0abc1de23f4567890");

    // The reported-requirement tail of each row is the member's pattern, unanchored.
    let rows: Vec<ConstraintRow> = vec![
        (
            "GetRecommendation with a recommendationIdentifier that is not an ARN",
            "GET",
            "/v1/recommendations/not-an-arn-at-all-padded-out".to_string(),
            None,
            "recommendationIdentifier",
            "not-an-arn-at-all-padded-out",
            RECOMMENDATION_IDENTIFIER_PATTERN,
        ),
        (
            "ListRecommendationResources with an account segment that is not 12 digits",
            "GET",
            format!(
                "/v1/recommendations/{}/resources?maxResults=50",
                label("arn:aws:trustedadvisor::nope:recommendation/r-1")
            ),
            None,
            "recommendationIdentifier",
            "arn:aws:trustedadvisor::nope:recommendation/r-1",
            RECOMMENDATION_IDENTIFIER_PATTERN,
        ),
        (
            "UpdateRecommendationLifecycle with a recommendation ARN of the wrong service",
            "PUT",
            format!(
                "/v1/recommendations/{}/lifecycle",
                label("arn:aws:support::000000000000:recommendation/r-1")
            ),
            Some(json!({
                "lifecycleStage": "in_progress",
                "updateReason": "a reason of a legal length",
                "updateReasonCode": "other",
            })),
            "recommendationIdentifier",
            "arn:aws:support::000000000000:recommendation/r-1",
            RECOMMENDATION_IDENTIFIER_PATTERN,
        ),
        (
            "GetOrganizationRecommendation with an account-level recommendation ARN",
            "GET",
            format!(
                "/v1/organization-recommendations/{}",
                label("arn:aws:trustedadvisor::000000000000:recommendation/r-1")
            ),
            None,
            "organizationRecommendationIdentifier",
            "arn:aws:trustedadvisor::000000000000:recommendation/r-1",
            ORGANIZATION_IDENTIFIER_PATTERN,
        ),
        (
            "ListOrganizationRecommendationResources with an organization ARN that is not one",
            "GET",
            format!(
                "/v1/organization-recommendations/{}/resources",
                label("arn:aws:trustedadvisor:::organization-recommendation-x")
            ),
            None,
            "organizationRecommendationIdentifier",
            "arn:aws:trustedadvisor:::organization-recommendation-x",
            ORGANIZATION_IDENTIFIER_PATTERN,
        ),
        (
            "ListOrganizationRecommendationAccounts with an affectedAccountId that is not digits",
            "GET",
            format!(
                "/v1/organization-recommendations/{organization}/accounts\
                 ?affectedAccountId=notadigits12"
            ),
            None,
            "affectedAccountId",
            "notadigits12",
            ACCOUNT_ID_PATTERN,
        ),
        (
            "ListRecommendationsForResource with an awsResourceArn that is not an ARN",
            "GET",
            "/v1/recommendations-for-resource/not-an-arn-at-all-padded-out".to_string(),
            None,
            "awsResourceArn",
            "not-an-arn-at-all-padded-out",
            AWS_RESOURCE_ARN_PATTERN,
        ),
        (
            "ListRecommendations with a checkIdentifier that is not a check ARN",
            "GET",
            format!(
                "/v1/recommendations?checkIdentifier={}",
                label("arn:aws:trustedadvisor:::checks/c-1")
            ),
            None,
            "checkIdentifier",
            "arn:aws:trustedadvisor:::checks/c-1",
            CHECK_PATTERN,
        ),
        (
            "ListRecommendationsForResource with a checkArn that is not a check ARN",
            "GET",
            format!(
                "/v1/recommendations-for-resource/{resource}?checkArn={}",
                label("arn:aws:trustedadvisor::000000000000:check/c-1")
            ),
            None,
            "checkArn",
            "arn:aws:trustedadvisor::000000000000:check/c-1",
            CHECK_PATTERN,
        ),
    ];

    for (case, method, path, body, member, value, pattern) in rows {
        let expected = pattern_message(member, value, pattern);
        assert_refused(&router, case, method, &path, body, &expected).await;
        assert!(
            !expected.contains('^') && !expected.contains('$'),
            "case {case:?}: the reported pattern must carry no anchors, got {expected:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: an enum's value set is listed in the order the service reports it
// ---------------------------------------------------------------------------

/// Every input member of this service that targets an enum, refused with a value outside
/// it. The rows differ along the enum, the operation, the member name and whether the
/// value rides in a query parameter or the body; an enum reached by SEVERAL members is
/// listed identically at each of them, because the order belongs to the shape.
#[tokio::test]
async fn an_enum_violation_lists_the_value_set_in_the_order_the_service_reports_it() {
    let router = emulator();
    let organization = label(ORGANIZATION_RECOMMENDATION);
    let priority = label(&recommendation_arn("low-utilization-ec2-instances"));
    let resource = label("arn:aws:ec2:us-east-1:000000000000:instance/i-0abc1de23f4567890");

    // The value set each enum is reported with. The first four are the recorded AWS
    // answers above; the rest carry no recorded order and so keep the order the model
    // declares (`smithy_query(query="shape",shape="RecommendationLanguage")` and siblings).
    let pillar = REPORTED_PILLAR;
    let source = REPORTED_SOURCE;
    let kind = REPORTED_TYPE;
    let status = REPORTED_STATUS;
    let language = "[en, ja, zh, fr, de, ko, zh_TW, it, es, pt_BR, id]";
    let exclusion = "[excluded, included]";
    let stage = "[pending_response, in_progress, dismissed, resolved]";
    let reason_code = "[non_critical_account, temporary_account, valid_business_case, \
                       other_methods_available, low_priority, not_applicable, other]";

    // The reported-requirement tail of each row is the enum's value set, as reported.
    let rows: Vec<ConstraintRow> = vec![
        (
            "ListChecks with a pillar outside RecommendationPillar",
            "GET",
            "/v1/checks?pillar=not_a_real_pillar".to_string(),
            None,
            "pillar",
            "not_a_real_pillar",
            pillar,
        ),
        (
            "ListRecommendations with a pillar outside RecommendationPillar",
            "GET",
            "/v1/recommendations?pillar=not_a_real_pillar".to_string(),
            None,
            "pillar",
            "not_a_real_pillar",
            pillar,
        ),
        (
            "ListRecommendationsForResource with a pillar outside RecommendationPillar",
            "GET",
            format!("/v1/recommendations-for-resource/{resource}?pillar=cost"),
            None,
            "pillar",
            "cost",
            pillar,
        ),
        (
            "ListChecks with a source outside RecommendationSource",
            "GET",
            "/v1/checks?source=not_a_real_source".to_string(),
            None,
            "source",
            "not_a_real_source",
            source,
        ),
        (
            "ListOrganizationRecommendations with a source outside RecommendationSource",
            "GET",
            "/v1/organization-recommendations?source=RDS".to_string(),
            None,
            "source",
            "RDS",
            source,
        ),
        (
            "ListRecommendations with a type outside RecommendationType",
            "GET",
            "/v1/recommendations?type=not_a_real_type".to_string(),
            None,
            "type",
            "not_a_real_type",
            kind,
        ),
        (
            "ListOrganizationRecommendations with a type outside RecommendationType",
            "GET",
            "/v1/organization-recommendations?type=Priority".to_string(),
            None,
            "type",
            "Priority",
            kind,
        ),
        (
            "ListRecommendations with a status outside RecommendationStatus",
            "GET",
            "/v1/recommendations?status=not-a-real-status".to_string(),
            None,
            "status",
            "not-a-real-status",
            status,
        ),
        (
            "ListRecommendationResources with a status outside ResourceStatus",
            "GET",
            format!("/v1/recommendations/{priority}/resources?status=OK"),
            None,
            "status",
            "OK",
            status,
        ),
        (
            "ListRecommendationResources with an exclusionStatus outside ExclusionStatus",
            "GET",
            format!("/v1/recommendations/{priority}/resources?exclusionStatus=all"),
            None,
            "exclusionStatus",
            "all",
            exclusion,
        ),
        (
            "ListChecks with a language outside RecommendationLanguage",
            "GET",
            "/v1/checks?language=en_GB".to_string(),
            None,
            "language",
            "en_GB",
            language,
        ),
        (
            "UpdateRecommendationLifecycle with a lifecycleStage outside its enum",
            "PUT",
            format!("/v1/recommendations/{priority}/lifecycle"),
            Some(json!({ "lifecycleStage": "not_a_stage" })),
            "lifecycleStage",
            "not_a_stage",
            stage,
        ),
        (
            "UpdateOrganizationRecommendationLifecycle with an updateReasonCode outside its enum",
            "PUT",
            format!("/v1/organization-recommendations/{organization}/lifecycle"),
            Some(json!({
                "lifecycleStage": "dismissed",
                "updateReasonCode": "because_i_said_so",
            })),
            "updateReasonCode",
            "because_i_said_so",
            reason_code,
        ),
    ];

    for (case, method, path, body, member, value, values) in rows {
        let expected = enum_message(member, value, values);
        assert_refused(&router, case, method, &path, body, &expected).await;
    }
}

// ---------------------------------------------------------------------------
// Rule 3: a `@sensitive` member's value is never echoed
// ---------------------------------------------------------------------------

/// `updateReason` is the one input member of this service whose target shape the model
/// marks `@sensitive` (`RecommendationUpdateReason`, `@length(10, 4096)`), so a clause
/// about it states the constraint with NO value. The rows differ along the operation, the
/// bound broken (minimum and maximum) and the value's size and alphabet — including one
/// that would otherwise be echoed over the framework's own echo cap.
#[tokio::test]
async fn a_sensitive_member_states_its_constraint_without_its_value() {
    let router = emulator();
    let organization = label(ORGANIZATION_RECOMMENDATION);
    let priority = label(&recommendation_arn("low-utilization-ec2-instances"));

    let too_short = ["short", "", "nine char", "héllo", "         "];
    let too_long = "x".repeat(4097);

    let mut rows: Vec<(String, &str, String, Value, &str)> = Vec::new();
    for reason in too_short {
        rows.push((
            format!("UpdateRecommendationLifecycle with updateReason {reason:?}"),
            "PUT",
            format!("/v1/recommendations/{priority}/lifecycle"),
            json!({ "lifecycleStage": "dismissed", "updateReason": reason, "updateReasonCode": "other" }),
            "Member must have length greater than or equal to 10",
        ));
        rows.push((
            format!("UpdateOrganizationRecommendationLifecycle with updateReason {reason:?}"),
            "PUT",
            format!("/v1/organization-recommendations/{organization}/lifecycle"),
            json!({ "lifecycleStage": "dismissed", "updateReason": reason }),
            "Member must have length greater than or equal to 10",
        ));
    }
    rows.push((
        "UpdateRecommendationLifecycle with an updateReason one byte past the maximum".to_string(),
        "PUT",
        format!("/v1/recommendations/{priority}/lifecycle"),
        json!({ "lifecycleStage": "dismissed", "updateReason": too_long, "updateReasonCode": "other" }),
        "Member must have length less than or equal to 4096",
    ));

    for (case, method, path, body, requirement) in rows {
        let expected = sensitive_message(requirement);
        assert_refused(&router, &case, method, &path, Some(body), &expected).await;
    }
}

/// The other side of that rule: a member the model does NOT mark `@sensitive` echoes the
/// value as submitted. The rows differ along the member, the bound broken (length and
/// range, each from both sides) and the value's size and type.
///
/// Every row's value is inside the framework's own 256-byte echo cap, the size at which
/// the value the framework's walk stores IS the submitted bytes. The same rule over values
/// PAST that cap — where the stored value is a shortened rendering and the submitted bytes
/// are read back off the request — is `tests/all/rejected_value_echo.rs`.
#[tokio::test]
async fn a_member_that_is_not_sensitive_echoes_the_value_as_submitted() {
    let router = emulator();
    let long_service = "s".repeat(31);
    let long_token = "t".repeat(250);

    // (case, path, member path, the value sent, the requirement it broke)
    let rows: Vec<(&str, String, &str, String, &str)> = vec![
        (
            "a nextToken shorter than the modelled minimum",
            "/v1/checks?nextToken=abc".to_string(),
            "nextToken",
            "abc".to_string(),
            "Member must have length greater than or equal to 4",
        ),
        (
            "an awsService shorter than the modelled minimum",
            "/v1/recommendations?awsService=e".to_string(),
            "awsService",
            "e".to_string(),
            "Member must have length greater than or equal to 2",
        ),
        (
            "an awsService one byte past the modelled maximum",
            format!("/v1/checks?awsService={long_service}"),
            "awsService",
            long_service.clone(),
            "Member must have length less than or equal to 30",
        ),
        (
            "a 250-byte awsService, far past the modelled maximum",
            format!("/v1/checks?awsService={long_token}"),
            "awsService",
            long_token.clone(),
            "Member must have length less than or equal to 30",
        ),
        (
            "a maxResults below the modelled range",
            "/v1/checks?maxResults=0".to_string(),
            "maxResults",
            "0".to_string(),
            "Member must have value greater than or equal to 1",
        ),
        (
            "a maxResults past the modelled range",
            "/v1/recommendations?maxResults=201".to_string(),
            "maxResults",
            "201".to_string(),
            "Member must have value less than or equal to 200",
        ),
    ];

    for (case, path, member, value, requirement) in rows {
        let expected = one_error(&format!(
            "Value '{value}' at '{member}' failed to satisfy constraint: {requirement}"
        ));
        assert_refused(&router, case, "GET", &path, None, &expected).await;
    }
}

// ---------------------------------------------------------------------------
// Rule 4: a collection member is stated as the collection the caller sent
// ---------------------------------------------------------------------------

/// One exclusion entry as a clause renders it: the shape's name, then each member it
/// carries as `<member>=<value>` in the order the model declares them (`arn`, then
/// `isExcluded`). Source:
/// `smithy_query(query="shape",shape="RecommendationResourceExclusion")`.
fn rendered_exclusion(arn: &str, is_excluded: bool) -> String {
    format!("RecommendationResourceExclusion(arn={arn}, isExcluded={is_excluded})")
}

/// `recommendationResourceExclusions` is the one list member of this service
/// (`@length(1, 25)`, `@uniqueItems`), and a clause about it states the list the caller
/// sent rather than a count of its elements. The rows differ along the element count
/// (past the maximum, far past it, and the empty list below the minimum) and along the
/// elements' own values, so neither the count nor one recorded batch is what the
/// rendering is read off.
#[tokio::test]
async fn a_list_violation_states_the_list_the_caller_sent() {
    let router = emulator();
    let path = "/v1/batch-update-recommendation-resource-exclusion";

    let mut rows: Vec<BatchRow> = Vec::new();
    for count in [26_usize, 30] {
        let entries: Vec<ExclusionEntry> = (0..count)
            .map(|index| {
                (
                    resource_arn("amazon-ebs-snapshots", &format!("res-ebs-{index:04}")),
                    // Alternating, so the boolean rendering is read off each element
                    // rather than off one value the whole batch shares.
                    index % 2 == 0,
                )
            })
            .collect();
        rows.push((
            format!("a batch of {count} entries, past the modelled maximum of 25"),
            entries,
            "Member must have length less than or equal to 25".to_string(),
        ));
    }
    rows.push((
        "an empty batch, below the modelled minimum of 1".to_string(),
        Vec::new(),
        "Member must have length greater than or equal to 1".to_string(),
    ));

    for (case, entries, requirement) in rows {
        let body = json!({
            "recommendationResourceExclusions": entries
                .iter()
                .map(|(arn, is_excluded)| json!({ "arn": arn, "isExcluded": is_excluded }))
                .collect::<Vec<Value>>(),
        });
        let rendered: Vec<String> = entries
            .iter()
            .map(|(arn, is_excluded)| rendered_exclusion(arn, *is_excluded))
            .collect();
        let expected = one_error(&format!(
            "Value '[{}]' at 'recommendationResourceExclusions' failed to satisfy \
             constraint: {requirement}",
            rendered.join(", ")
        ));
        assert_refused(&router, &case, "PUT", path, Some(body), &expected).await;
    }

    // The bound itself: a batch AT the maximum is not a validation failure at all, so the
    // rendering above is about the rule and not about every batch.
    let at_the_bound = json!({
        "recommendationResourceExclusions": (0..25)
            .map(|index| json!({
                "arn": resource_arn("amazon-ebs-snapshots", &format!("res-ebs-{index:04}")),
                "isExcluded": true,
            }))
            .collect::<Vec<Value>>(),
    });
    let (status, _errortype, response) =
        dispatch(&router, "PUT", path, Some(at_the_bound)).await;
    assert_ne!(
        status, 400,
        "case a batch of 25 entries, at the modelled maximum: expected the operation's \
         own answer, got {status} with {response}"
    );
}

/// The bound on the rendering itself: a batch whose rendering is larger than this
/// service's stated ceiling keeps the framework's own `<n> items` summary, so one request
/// can never be re-echoed into an arbitrarily long 400 body. Each ARN here is just inside
/// `RecommendationResourceArn`'s `@length(20, 2048)` and inside its pattern, so the list
/// bound is still the only rule broken.
#[tokio::test]
async fn a_rendering_past_the_stated_ceiling_keeps_the_frameworks_summary() {
    let router = emulator();
    let entries: Vec<Value> = (0..26)
        .map(|index| {
            json!({
                "arn": format!(
                    "arn:aws:trustedadvisor::{}:recommendation-resource/batch/{index:04}-{}",
                    engine::EMULATOR_ACCOUNT_ID,
                    "a".repeat(1_900),
                ),
                "isExcluded": true,
            })
        })
        .collect();
    let expected = one_error(
        "Value '26 items' at 'recommendationResourceExclusions' failed to satisfy \
         constraint: Member must have length less than or equal to 25",
    );
    assert_refused(
        &router,
        "a batch of 26 entries whose rendering is past the stated ceiling",
        "PUT",
        "/v1/batch-update-recommendation-resource-exclusion",
        Some(json!({ "recommendationResourceExclusions": entries })),
        &expected,
    )
    .await;
}

/// A `@required` list member absent from the body is the same refusal to a caller as a
/// value outside a bound, and is stated the same way: the member named, and the null it
/// was in place of a value.
#[tokio::test]
async fn an_absent_required_member_is_stated_as_the_null_it_was() {
    let router = emulator();
    let rows = [
        (
            "BatchUpdateRecommendationResourceExclusion with an empty body",
            "PUT",
            "/v1/batch-update-recommendation-resource-exclusion".to_string(),
            json!({}),
            "recommendationResourceExclusions",
        ),
        (
            "UpdateRecommendationLifecycle with no lifecycleStage",
            "PUT",
            format!(
                "/v1/recommendations/{}/lifecycle",
                label(&recommendation_arn("low-utilization-ec2-instances"))
            ),
            json!({ "updateReasonCode": "other" }),
            "lifecycleStage",
        ),
    ];

    for (case, method, path, body, member) in rows {
        let expected = one_error(&format!(
            "Value null at '{member}' failed to satisfy constraint: Member must not be null"
        ));
        assert_refused(&router, case, method, &path, Some(body), &expected).await;
    }
}

// ---------------------------------------------------------------------------
// Rule 5: the envelope counts and joins the clauses
// ---------------------------------------------------------------------------

/// Two members of one request broken at once: the envelope counts the clauses
/// (`<N> validation errors detected: `, plural) and joins them with `; `. The clause
/// ORDER is the boundary walk's, which this suite does not pin — what it pins is that
/// both members are reported, once each, in the counted envelope.
#[tokio::test]
async fn several_violations_are_counted_and_joined_in_one_envelope() {
    let router = emulator();
    let (status, errortype, response) = dispatch(
        &router,
        "GET",
        "/v1/checks?pillar=not_a_real_pillar&maxResults=0",
        None,
    )
    .await;
    let case = "ListChecks with both a pillar outside its enum and a maxResults below its range";
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {response}"
    );
    assert_eq!(
        errortype.as_deref(),
        Some("ValidationException"),
        "case {case:?}: expected x-amzn-errortype ValidationException, got {errortype:?}"
    );
    let message = message_of(&response);
    assert!(
        message.starts_with("2 validation errors detected: "),
        "case {case:?}: expected the plural envelope over two clauses, got {message:?}"
    );
    let clauses: Vec<&str> = message
        .trim_start_matches("2 validation errors detected: ")
        .split("; ")
        .collect();
    assert_eq!(
        clauses.len(),
        2,
        "case {case:?}: expected two clauses joined with \"; \", got {message:?}"
    );
    for expected in [
        "Value 'not_a_real_pillar' at 'pillar' failed to satisfy constraint: Member must \
         satisfy enum value set: [security, service_limits, cost_optimizing, \
         operational_excellence, fault_tolerance, performance]",
        "Value '0' at 'maxResults' failed to satisfy constraint: Member must have value \
         greater than or equal to 1",
    ] {
        assert!(
            clauses.contains(&expected),
            "case {case:?}: expected a clause {expected:?}, got {message:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Every operation the model binds is worded by this service
// ---------------------------------------------------------------------------

/// The rules above are properties of the INPUT SHAPE, and `ValidationException` is a
/// common error of every operation this service binds, so they must hold for ALL twelve
/// (`OPERATIONS.md`) — implemented or still scaffolded.
///
/// This table is the exhaustiveness check for that: one refused request per operation,
/// each expecting a message the framework's own renderer would NOT produce (an unanchored
/// pattern, an enum in the service's order, a `@sensitive` member with no value, or a
/// rendered list). So an operation the model gains later, and which is not served through
/// this service's refusal path, fails here instead of silently falling back to the
/// framework's wording for that one operation.
#[tokio::test]
async fn every_operation_the_model_binds_is_worded_by_this_service() {
    let router = emulator();
    let organization = label(ORGANIZATION_RECOMMENDATION);
    let priority = label(&recommendation_arn("low-utilization-ec2-instances"));
    let malformed = "not-an-arn-at-all-padded-out";
    let short_reason = json!({ "lifecycleStage": "dismissed", "updateReason": "short" });
    let batch: Vec<ExclusionEntry> = (0..26)
        .map(|index| {
            (
                resource_arn("amazon-ebs-snapshots", &format!("res-ebs-{index:04}")),
                true,
            )
        })
        .collect();
    let rendered_batch: Vec<String> = batch
        .iter()
        .map(|(arn, is_excluded)| rendered_exclusion(arn, *is_excluded))
        .collect();

    // (operation, method, path, body, the message it must be refused with)
    let rows: Vec<(&str, &str, String, Option<Value>, String)> = vec![
        (
            "ListChecks",
            "GET",
            "/v1/checks?pillar=not_a_real_pillar".to_string(),
            None,
            enum_message("pillar", "not_a_real_pillar", REPORTED_PILLAR),
        ),
        (
            "ListRecommendations",
            "GET",
            "/v1/recommendations?source=not_a_real_source".to_string(),
            None,
            enum_message("source", "not_a_real_source", REPORTED_SOURCE),
        ),
        (
            "ListOrganizationRecommendations",
            "GET",
            "/v1/organization-recommendations?type=not_a_real_type".to_string(),
            None,
            enum_message("type", "not_a_real_type", REPORTED_TYPE),
        ),
        (
            "GetRecommendation",
            "GET",
            format!("/v1/recommendations/{malformed}"),
            None,
            pattern_message(
                "recommendationIdentifier",
                malformed,
                RECOMMENDATION_IDENTIFIER_PATTERN,
            ),
        ),
        (
            "ListRecommendationResources",
            "GET",
            format!("/v1/recommendations/{priority}/resources?status=not-a-real-status"),
            None,
            enum_message("status", "not-a-real-status", REPORTED_STATUS),
        ),
        (
            "UpdateRecommendationLifecycle",
            "PUT",
            format!("/v1/recommendations/{priority}/lifecycle"),
            Some(short_reason.clone()),
            sensitive_message("Member must have length greater than or equal to 10"),
        ),
        (
            "GetOrganizationRecommendation",
            "GET",
            format!("/v1/organization-recommendations/{malformed}"),
            None,
            pattern_message(
                "organizationRecommendationIdentifier",
                malformed,
                ORGANIZATION_IDENTIFIER_PATTERN,
            ),
        ),
        (
            "ListOrganizationRecommendationAccounts",
            "GET",
            format!(
                "/v1/organization-recommendations/{organization}/accounts\
                 ?affectedAccountId=notadigits12"
            ),
            None,
            pattern_message("affectedAccountId", "notadigits12", ACCOUNT_ID_PATTERN),
        ),
        (
            "ListOrganizationRecommendationResources",
            "GET",
            format!("/v1/organization-recommendations/{organization}/resources?status=OK"),
            None,
            enum_message("status", "OK", REPORTED_STATUS),
        ),
        (
            "UpdateOrganizationRecommendationLifecycle",
            "PUT",
            format!("/v1/organization-recommendations/{organization}/lifecycle"),
            Some(short_reason),
            sensitive_message("Member must have length greater than or equal to 10"),
        ),
        (
            "ListRecommendationsForResource",
            "GET",
            format!("/v1/recommendations-for-resource/{malformed}"),
            None,
            pattern_message("awsResourceArn", malformed, AWS_RESOURCE_ARN_PATTERN),
        ),
        (
            "BatchUpdateRecommendationResourceExclusion",
            "PUT",
            "/v1/batch-update-recommendation-resource-exclusion".to_string(),
            Some(json!({
                "recommendationResourceExclusions": batch
                    .iter()
                    .map(|(arn, is_excluded)| json!({ "arn": arn, "isExcluded": is_excluded }))
                    .collect::<Vec<Value>>(),
            })),
            one_error(&format!(
                "Value '[{}]' at 'recommendationResourceExclusions' failed to satisfy \
                 constraint: Member must have length less than or equal to 25",
                rendered_batch.join(", ")
            )),
        ),
    ];

    // The whole surface, so a thirteenth operation cannot be added to the model without
    // this table being extended too (`OPERATIONS.md` states the count).
    assert_eq!(
        rows.len(),
        12,
        "this table must cover every operation the model binds, got {} rows",
        rows.len()
    );

    for (operation, method, path, body, expected) in rows {
        assert_refused(&router, operation, method, &path, body, &expected).await;
    }
}
