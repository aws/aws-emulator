Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: b4093aae6d988631adde420a6428194a1a0d4714

# An organization-level preference scope is accepted from a caller that manages no organization

_Filed: 2026-11-20T07:24:04Z_
_Operation: PutRecommendationPreferences | Class: validation_

> _Note: this issue was identified against the code base at commit `b4093aae6d988631adde420a6428194a1a0d4714`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model documents that organization-level recommendation preferences may only be created by a management account: `PutRecommendationPreferences$scope` says "You can create recommendation preferences at the organization level (for management accounts of an organization only), account level, and resource level." The handler's `request_rejection` implements the OTHER restriction from that same member's documentation (the Auto Scaling group resource-level-only note) but never checks the organization-level one, and `crate::state::scope_rejection` only validates that an `Organization` scope's value is `ALL_ACCOUNTS`.

So a standalone caller — which is what this emulator presents whenever the store records no organization members — can write a preference at `scope = {name: "Organization", value: "ALL_ACCOUNTS"}`. That record is then inherited by every resource of the account through `account_scope_precedence`, which ranks an `Organization` key as applicable (rank 3), so the preference silently takes effect for a caller the model says cannot create it.

The crate already owns the test for this condition and applies it everywhere else an organization-wide surface is served: `manages_an_organization()` plus the single shared refusal `INCLUDE_MEMBER_ACCOUNTS_MESSAGE` are used by `UpdateEnrollmentStatus`, by the shared export admission rule and by `GetEnrollmentStatusesForOrganization`. The preference writer is the one path that needs it and skips it.

## Evidence

Model sentence (smithy_query doc, `PutRecommendationPreferencesRequest$scope`):
> "You can create recommendation preferences at the organization level (for management accounts of an organization only), account level, and resource level."
(The same parenthetical appears on `DeleteRecommendationPreferencesRequest$scope` and `GetRecommendationPreferencesRequest$scope`.)

The handler implements the sibling sentence from that same doc note but not this one — `src/ops/put_recommendation_preferences.rs:320-331`:
```
    // `scope`: "You cannot create recommendation preferences for Auto Scaling groups at
    // the organization and account levels. ..."
    if input.resource_type == crate::state::RESOURCE_TYPE_AUTO_SCALING_GROUP
        && scope_name != Some(crate::state::SCOPE_RESOURCE_ARN)
    {
        return Some(crate::state::AUTO_SCALING_GROUP_SCOPE_MESSAGE.to_string());
    }
```
and the only gate taken after the store lock is the opt-in one — `src/ops/put_recommendation_preferences.rs:242-254`:
```
        let mut guard = state.lock();
        if !guard.enrollment.is_opted_in() { ... OptInRequiredException ... }
```
Scope validation checks only the value — `src/state.rs:1078-1083`:
```
        (Some(SCOPE_ORGANIZATION), Some(value)) if value != ORGANIZATION_SCOPE_VALUE => {
            Some(format!(... must be {ORGANIZATION_SCOPE_VALUE}, not {value}."))
        }
```
The helper the crate already has, and the siblings that use it — `src/state.rs:4051-4052`:
```
    pub fn manages_an_organization(&self) -> bool {
        !self.organization_members.is_empty()
    }
```
`src/ops/update_enrollment_status.rs:218`: `if let Some(message) = guard.include_member_accounts_rejection(input.include_member_accounts)`; `src/state/export.rs:511`: same call in the shared export admission; `src/ops/get_enrollment_statuses_for_organization.rs:253-255`: `if !guard.manages_an_organization() { return Err(not_a_management_account()); }`.
The stored organization-level record is inherited by every resource — `src/state.rs:1117-1123` (`account_scope_precedence` returns `Some(3)` for `(Some(SCOPE_ORGANIZATION), _)`).
Test coverage: `tests/all/enrollment_and_preferences.rs:500-522` and `tests/all/unrestricted_preference_resource_types.rs:97-119` write `{"name": "Organization", "value": "ALL_ACCOUNTS"}` through `put_preference` (`tests/all/harness.rs:1025-1030`, which asserts 200) with no organization recorded; neither test states a reason for admitting it. `tests/all/include_member_accounts.rs:112` and `tests/all/shared_paging_rules.rs:624` show the harness that records organization members.

## Steps to Reproduce

1. Start the emulator with no organization members recorded (the default standalone account).
2. Call UpdateEnrollmentStatus with status=Active to opt in.
3. Call UpdateEnrollmentStatus with includeMemberAccounts=true: it is refused with InvalidParameterValueException and INCLUDE_MEMBER_ACCOUNTS_MESSAGE, proving the service judges this caller to manage no organization.
4. Call PutRecommendationPreferences with `{"resourceType": "Ec2Instance", "scope": {"name": "Organization", "value": "ALL_ACCOUNTS"}, "lookBackPeriod": "DAYS_93"}`.
5. Expected: refused — the model states organization-level preferences are for management accounts of an organization only, which this caller is not.
6. Actual: HTTP 200 and the record is stored; GetRecommendationPreferences reports it and GetEffectiveRecommendationPreferences for any instance ARN now reports lookBackPeriod=DAYS_93 inherited from the organization scope.
