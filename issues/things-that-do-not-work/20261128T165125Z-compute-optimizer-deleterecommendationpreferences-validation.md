Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: b60f6fc1d7d48b8ab01ee41fe8076d8fc438ec86

# An organization-level scope is accepted from a caller that manages no organization, although the scope member documents the organization level as management-accounts-only and the sibling writer refuses it

_Filed: 2026-11-28T16:51:25Z_
_Operation: DeleteRecommendationPreferences | Class: validation_

> _Note: this issue was identified against the code base at commit `b60f6fc1d7d48b8ab01ee41fe8076d8fc438ec86`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteRecommendationPreferences$scope` documents the organization level as available to management accounts only, exactly as `PutRecommendationPreferences$scope` does. The crate states that caller test once, in `state.rs`, as `organization_scope_rejection`, whose doc describes it as the rule for "a `scope` a preference WRITE carries" — and `PutRecommendationPreferences` is the only handler that calls it.

`DeleteRecommendationPreferences` is the crate's OTHER preference-write path (it mutates `guard.preferences`, clearing named preferences and removing the record) and it never asks that question: it validates the scope's well-formedness with the shared `scope_rejection` and then addresses `PreferenceKey::new(&input.resource_type, input.scope.as_ref())` directly. So a caller that manages no organization is answered 200 for a delete at a scope the model says it cannot operate at, while the identical scope on the sibling write is refused 400 — and the handler's own comment claims it orders its refusals "as it does in the other three preference operations".

## Evidence

Smithy, DeleteRecommendationPreferencesRequest$scope (smithy_query query="doc"): "You can delete recommendation preferences that are created at the organization level (for management accounts of an organization only), account level, and resource level."

The crate's single home for that caller test, documented as the rule for a preference write — src/state.rs:4094-4111:
```
    /// Why a `scope` a preference WRITE carries cannot be honoured, or `None` when it can.
    ///
    /// An organization-level scope ([`SCOPE_ORGANIZATION`]) states a preference for every
    /// account of an organization, which the model admits for one caller only: "You can create
    /// recommendation preferences at the organization level (for management accounts of an
    /// organization only), account level, and resource level"
    /// (`PutRecommendationPreferences$scope`). A caller that manages no organization
    /// ... has no organization to set a
    /// preference for, so the write is refused ...
    pub fn organization_scope_rejection(&self, scope: Option<&Scope>) -> Option<&'static str> {
```

The sibling write calls it — src/ops/put_recommendation_preferences.rs:255-264:
```
        if let Some(message) = guard.organization_scope_rejection(input.scope.as_ref()) {
            return Err(
                PutRecommendationPreferencesErrorModeled::InvalidParameterValueException(
```

This write does not — src/ops/delete_recommendation_preferences.rs:176-236 is the whole validation and mutation; it calls only `scope_rejection` (line 181) and `is_recommendation_preference_name` (line 194), then mutates:
```
        let key = crate::state::PreferenceKey::new(&input.resource_type, input.scope.as_ref());
        if let Some(record) = guard.preferences.get_mut(&key) {
            for name in &input.recommendation_preference_names {
                record.clear_preference(name);
            }
```
while its own comment at delete_recommendation_preferences.rs:176-180 claims parity: "InvalidParameterValueException therefore also takes precedence over the opt-in gate below, as it does in the other three preference operations." (`grep organization_scope_rejection` over the crate returns put_recommendation_preferences.rs:255 and state.rs:4106 only.)

The current behaviour is pinned by a test whose stated reason the model sentence contradicts — tests/all/enrollment_and_preferences.rs:1688-1729:
```
/// Reading and deleting at the organization level stay SERVED to a caller that manages no
/// organization: the rule is about creating that record, and such a caller holds none — so the
/// read reports nothing at that scope and the delete removes nothing ...
async fn an_organization_level_read_and_delete_report_the_records_the_caller_holds() {
...
    assert_eq!(
        status, 200,
        "case \"the organization-level delete\": expected 200, got {status}: {body}"
```
The member's own sentence is about DELETING ("You can delete recommendation preferences that are created at the organization level (for management accounts of an organization only)"), so "the rule is about creating that record" is not what the model says for this operation.

## Steps to Reproduce

1. Mount the emulator as the default standalone account (no organization member accounts recorded) and opt in with UpdateEnrollmentStatus {"status": "Active", "includeMemberAccounts": false}.
2. Call PutRecommendationPreferences with {"resourceType": "Ec2Instance", "scope": {"name": "Organization", "value": "ALL_ACCOUNTS"}, "lookBackPeriod": "DAYS_93"} — 400 InvalidParameterValueException (put_recommendation_preferences.rs:255, state.rs:4106).
3. Call DeleteRecommendationPreferences with {"resourceType": "Ec2Instance", "scope": {"name": "Organization", "value": "ALL_ACCOUNTS"}, "recommendationPreferenceNames": ["LookBackPeriodPreference"]} — 200 with an empty body.
4. Expected the same 400 the sibling write answers for the same scope from the same caller, since the member documents the organization level as "for management accounts of an organization only" for the delete too.
