Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: b60f6fc1d7d48b8ab01ee41fe8076d8fc438ec86

# An OrganizationRule preview that names no organizationScope is served an organization-wide page, although the member documents it as required for that rule type

_Filed: 2026-11-28T16:52:23Z_
_Operation: ListAutomationRulePreview | Class: validation_

> _Note: this issue was identified against the code base at commit `b60f6fc1d7d48b8ab01ee41fe8076d8fc438ec86`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Both preview operations share `crate::state::preview_selection_violation`, which judges the `ruleType`/`organizationScope` pair in ONE direction only: it refuses an `organizationScope` sent with a rule type other than `OrganizationRule`, and says nothing about an `OrganizationRule` request that sends no scope at all. The model documents the member as REQUIRED for exactly that rule type, so `{"ruleType":"OrganizationRule","recommendedActionTypes":[...]}` with no `organizationScope` must be refused; instead `crate::state::preview_account_scope` turns the absent member into the all-covering `AccountScope::default()` and the call is answered 200 with a preview spanning every account the caller reaches (and `ListAutomationRulePreviewSummaries`, which calls the same two helpers, aggregates that same organization-wide selection).

The crate already implements the mirror-image rule for the rule WRITE path and claims parity with the preview path while doing so: `crate::state::organization_configuration_violation` refuses BOTH directions of the identical pairing ('Member is required for the OrganizationRule rule type' / 'Member is valid only for the OrganizationRule rule type'), and its own doc comment asserts that the preview check is 'the same reading'. It is not: of the two clauses the rule path applies, the preview path applies only the second.

## Evidence

Model (kind A), `smithy_query(query="doc",shape="ListAutomationRulePreviewRequest",member="organizationScope")`:
> "The organizational scope for the rule preview. This parameter is required when `ruleType` is `OrganizationRule`. Don't specify it when `ruleType` is `AccountRule`."
The same sentence is on `ListAutomationRulePreviewSummariesRequest$organizationScope` (`smithy_query(query="doc",shape="ListAutomationRulePreviewSummariesRequest",member="organizationScope")`).

The shared check that enforces only the second half — services/service-com-amazonaws-computeoptimizerautomation-computeoptimizerautomationservice/src/state.rs:1794-1803:
```
    if let Some(report) = recommended_action_types_violation(recommended_action_types) {
        return Some(report);
    }
    if rule_type != "OrganizationRule" && organization_scope.is_some() {
        return Some(constraint_report(
            "organizationScope",
            "Member is valid only for the OrganizationRule rule type",
        ));
    }
    criteria.and_then(criteria_violation)
```
(no clause for `rule_type == "OrganizationRule" && organization_scope.is_none()`).

What the absent member is turned into instead — src/state.rs:2599-2604:
```
    if rule_type != "OrganizationRule" {
        return AccountScope::of_accounts(vec![caller_account_id()]);
    }
    let Some(scope) = organization_scope else {
        return AccountScope::default();
    };
```
with its doc at src/state.rs:2585-2588: "An `OrganizationRule` preview is scoped by its `organizationScope` … and one that names no scope at all spans the organization." `AccountScope::default()` answers `includes_everything()` (src/state.rs:356-358), so every retrievable account's actions are previewed.

The repo's other side of the same rule — src/state.rs:3227-3240:
```
    match (
        rule_type == "OrganizationRule",
        organization_configuration.is_some(),
    ) {
        (true, false) => Some(constraint_report(
            "organizationConfiguration",
            "Member is required for the OrganizationRule rule type",
        )),
        (false, true) => Some(constraint_report(
            "organizationConfiguration",
            "Member is valid only for the OrganizationRule rule type",
        )),
```
and its doc claiming parity with the preview path (src/state.rs:3214-3218): "the same reading [`preview_selection_violation`] refuses an `organizationScope` outside that rule type by".

The two handlers that call the one-sided check: src/ops/list_automation_rule_preview.rs:201-208 and src/ops/list_automation_rule_preview_summaries.rs:206-213.

Test coverage: tests/all/preview_selection_guard.rs exercises the `organizationScope`-with-AccountRule refusal (lines 160-174) and has no row for an OrganizationRule request that omits the scope. Two suites currently send exactly that request and expect 200 — tests/all/disassociated_member_retrieval.rs:116-125 (`preview_request(None)` builds `{"ruleType":"OrganizationRule","recommendedActionTypes":[...]}`, asserted at lines 264-277 as "A preview naming no scope spans the organization") and tests/all/page_size_rule.rs:141-146 — so those rows are what the quoted model sentence contradicts.

## Steps to Reproduce

1. Mount the service and open the enrollment gate: UpdateEnrollmentConfiguration {"status":"Active"}.
2. AssociateAccounts {"accountIds":["777788889999"],"clientToken":"t1"} so more than the caller's own account has analysed recommended actions.
3. Call ListAutomationRulePreview {"ruleType":"OrganizationRule","recommendedActionTypes":["UpgradeEbsVolumeType","SnapshotAndDeleteUnattachedEbsVolume"]} — no organizationScope.
4. Documented: the member "is required when ruleType is OrganizationRule", so the request is refused with the operation's only client-input error (InvalidParameterValueException, HTTP 400), the way CreateAutomationRule refuses an OrganizationRule carrying no organizationConfiguration. Actual: HTTP 200 with previewResults covering every account the caller reaches, including 777788889999.
5. Repeat with ListAutomationRulePreviewSummaries and the same body: it too answers 200, with an aggregate row totalling the organization-wide selection.
6. For contrast, CreateAutomationRule {"name":"r","ruleType":"OrganizationRule","recommendedActionTypes":[...],"schedule":{...},"status":"Active"} with no organizationConfiguration is refused 400 'Member is required for the OrganizationRule rule type' — the clause the preview path omits.
