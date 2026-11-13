Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: f0e24d1fa06c2873d160f000b4fc1f33ca2ae14b

# Malformed-nextToken ValidationException is worded ad hoc in all seven list handlers instead of through the crate's refusal-sentence builder

_Filed: 2026-10-31T19:36:38Z_
_Operation: ListChecks | Class: error-response_

> _Note: this issue was identified against the code base at commit `f0e24d1fa06c2873d160f000b4fc1f33ca2ae14b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate states ONE rule for the `message` of a `ValidationException` this service raises itself: a condition the model's constraint traits cannot express is refused by the handler that owns it, and the SENTENCE for that refusal is built through `crate::service::validation`'s `clause` / `echoed_value` / `refusal_sentence` so the handler-raised path and the framework-collected path cannot disagree. `BatchUpdateRecommendationResourceExclusion` follows that rule for its repeated-ARN condition.

The other handler-raised input-validation refusal of this crate — a `nextToken` that is not a cursor this read issued, a condition no constraint trait can express — breaks it. All seven paginated handlers map `crate::state::MalformedToken` to a `ValidationException` whose `message` is a hand-written bare sentence ("nextToken is not a pagination token this service issued for this list"), with no `<N> validation error(s) detected: ` envelope and no `<subject> at '<member path>' failed to satisfy constraint: <requirement>` clause. So the same service answers two of its own handler-raised refusals in two different sentence shapes, and the seven copies can drift from each other as well.

Affected handlers (identical ad-hoc message): ListChecks, ListRecommendations, ListRecommendationResources, ListOrganizationRecommendations, ListOrganizationRecommendationAccounts, ListOrganizationRecommendationResources, ListRecommendationsForResource. `ListChecks` is named here because it is the first operation on the checklist whose handler words the shared `crate::state::paginate` / `decode_token` failure.

## Evidence

The rule, in the module that owns the words — services/service-com-amazonaws-trustedadvisor-trustedadvisor/src/service/validation.rs:47-53:
```
//! A condition the model's constraint traits CANNOT express is refused by the handler that
//! owns it, so the framework's walk never collects it and it reaches no [`Violation`]. Such
//! a refusal is still one of this service's input-validation refusals and is worded here
//! too: the handler states its own subject, member path and requirement and builds the
//! sentence through [`clause`], [`echoed_value`] and [`refusal_sentence`], which is why
//! those three are the crate's and not this module's alone. Nothing about the CONDITION
//! moves here — only the sentence.
```
and the builders it exports crate-wide — validation.rs:227-253: `pub(crate) fn clause(...)`, `pub(crate) fn echoed_value(...)`, `pub(crate) fn refusal_sentence(...)` producing `"{} validation errors detected: {}"` / `"1 validation error detected: {only}"`.

The sibling that obeys it — src/ops/batch_update_recommendation_resource_exclusion.rs:107-110:
```
// The module that owns the WORDS of this service's input-validation refusals. This
// handler raises one of its own (see `repeated_arn_refusal`) and builds its sentence
// there rather than spelling the clause and the envelope a second time.
use crate::service::validation;
```
and :151-163 `fn repeated_arn_refusal(...) -> Option<String>` → `validation::clause(&validation::echoed_value(arn), ...)` + `validation::refusal_sentence(&clauses)`.

The seven handlers that do not — e.g. src/ops/list_checks.rs:243-248:
```
.map_err(|_| {
    ListChecksErrorModeled::ValidationException(ValidationException {
        message: "nextToken is not a pagination token this service issued for this list"
            .to_string(),
    })
})?;
```
Same literal at src/ops/list_recommendations.rs:321-326, src/ops/list_recommendation_resources.rs:280-285, src/ops/list_organization_recommendations.rs:311-316, src/ops/list_organization_recommendation_accounts.rs:279-287, src/ops/list_organization_recommendation_resources.rs:316-324, src/ops/list_recommendations_for_resource.rs:325-330.

That this is a HANDLER-raised refusal is stated by the suite itself — tests/all/recommendation_loop.rs:377-379: "each with a token that is not base64 of a key (the modelled `length 4..10000` is satisfied, so the refusal is the handler's own)".

Test coverage: tests/all/recommendation_loop.rs:373-469, tests/all/cursor_scope_encoding.rs, tests/all/organization_recommendation_accounts.rs:443-530, tests/all/organization_recommendation_resources.rs:634-721 and tests/all/organization_recommendations.rs:588-656 all assert only HTTP 400 plus `error_code(&body) == "ValidationException"` and never read `message`, while tests/all/refusal_sentence_one_builder.rs:121-170 parses the envelope and clause skeleton for the two paths it drives (framework-collected, and the batch handler's repeated-ARN rule) — the nextToken path is in neither row set.

## Steps to Reproduce

1. Start the emulator with EMULATOR_TRUSTEDADVISOR_SUPPORT_PLAN=business so the support-level gate passes.
2. GET /v1/batch-update-recommendation-resource-exclusion's sibling refusal first: PUT /v1/batch-update-recommendation-resource-exclusion with the same recommendation-resource ARN twice. Observe 400 ValidationException whose message is `1 validation error detected: Value '<arn>' at 'recommendationResourceExclusions' failed to satisfy constraint: Member must have unique arn values`.
3. Now GET /v1/checks?nextToken=%21%21%21%21 (four characters, so the modelled @length(4,10000) is satisfied and the refusal is the handler's).
4. Observe 400 ValidationException whose message is the bare `nextToken is not a pagination token this service issued for this list` — no `N validation error(s) detected: ` envelope and no `<subject> at '<member path>' failed to satisfy constraint: <requirement>` clause, contradicting the rule src/service/validation.rs:47-53 states for exactly this class of refusal.
5. Repeat step 3 against /v1/recommendations, /v1/recommendations/{arn}/resources, /v1/organization-recommendations, /v1/organization-recommendations/{arn}/accounts, /v1/organization-recommendations/{arn}/resources and /v1/recommendations-for-resource/{arn} to see the same ad-hoc sentence from six more copies.
