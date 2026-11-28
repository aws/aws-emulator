Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 447941bda47a9e01bc13409633bd0ec832671375

# An EventStatus filter value outside the set the member documents as valid is accepted and answered with an authoritative-looking page, the one outcome this crate's own filter rule exists to prevent

_Filed: 2026-11-28T13:44:48Z_
_Operation: ListAutomationEvents | Class: validation_

> _Note: this issue was identified against the code base at commit `447941bda47a9e01bc13409633bd0ec832671375`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`AutomationEventFilter$values` enumerates, in prose, the values the `EventStatus` filter may carry. The model cannot express that set as a constraint — the member targets `FilterValues`, whose member is a plain `FilterValue` bounded only by `@length(1..255)` and the pattern `^[a-zA-Z0-9\-_\.\*\?\s]+$` — so the generated request walk admits any value of that form, and the handler adds no check: `ListAutomationEvents` only asks `crate::state::filters_violation` whether a filter carries NO value, then hands the list to `crate::state::event_matches_filters`, which compares the stored `eventStatus` for string equality. A request such as `{"filters":[{"name":"EventStatus","values":["Done"]}]}` is therefore answered HTTP 200 with an empty `automationEvents` list and no `nextToken` — which a client reads as the authoritative statement that the account has no event in that status, rather than as "that is not a status of this service".

That is precisely the outcome this crate's own filter rule is written to prevent: `filters_violation` refuses a filter carrying no value because such a filter "states a condition nothing can satisfy" and must not be "answered with an empty page that reads as an authoritative result set". A value outside the documented set states exactly the same unsatisfiable condition and is admitted by the same boundary for the same reason, yet is not judged.

The crate already implements this shape of rule for the other member whose valid values the model gives in prose only: `UpdateEnrollmentConfiguration` refuses a `status` outside `SETTABLE_ENROLLMENT_STATUSES` in the handler, with the explicit reasoning that both excluded values are "legal members of `EnrollmentStatus`, so the protocol boundary admits them and the refusal belongs here". The value set needed here is already generated into the crate as `EVENT_STATUS_VALUES`, listing exactly the nine values the member's prose lists.

The reason the gap was never seen is recorded in `REQUEST_ENUM_TYPES`' own doc, which asserts that `EventStatus` (and `EventType`, `ResourceType`) "appear in RESPONSES only, so no request can be refused against one". `AutomationEventFilter$values` contradicts that: a request does carry an `EventStatus` value, and the member documents which ones are valid.

`ListAutomationEventSummaries` reaches the same helper with the same filter list and so has the same behaviour; `ListAutomationEvents` is the first operation whose handler calls it.

## Evidence

The modelled rule, verbatim — `smithy_query(query="doc",shape="AutomationEventFilter",member="values")`:

> "The values to use for the specified filter. For the `ResourceType`, `EventType`, and `EventStatus` filters, the value that you specify must exactly match the corresponding field of the automation event, including case. For example, specify `Complete`, not `complete`. Wildcards aren't supported. An asterisk (`*`) or a question mark (`?`) in a value matches only that literal character.
> **Valid values for the `EventStatus` filter are `Ready`, `InProgress`, `Complete`, `Failed`, `Cancelled`, `RollbackReady`, `RollbackInProgress`, `RollbackComplete`, and `RollbackFailed`.**"

The only judgement the handler makes over a filter is the empty-value floor —
services/service-com-amazonaws-computeoptimizerautomation-computeoptimizerautomationservice/src/ops/list_automation_events.rs:172
```
        // `AutomationEventFilter$name` is typed as the `AutomationEventFilterName`
        // value set, so the generated request walk has already refused a name
        // outside it — naming the offending filter by its position in the list —
        // and every name that gets here is one
        // `crate::state::event_matches_filters` selects on.
        let filters = input.filters.unwrap_or_default();

        // A filter carrying no value selects nothing, and the boundary admits
        // one: `crate::state::filters_violation` is where this service judges
        // that, for every operation that filters, and why.
        if let Some(report) = crate::state::filters_violation(&filters) {
            return Err(invalid_parameter(report));
        }
```
…and the evaluator then matches by plain equality, so an out-of-set value simply matches nothing —
services/.../src/state.rs:3478
```
pub fn event_matches_filters(event: &StoredEvent, filters: &[AutomationEventFilter]) -> bool {
    filters.iter().all(|filter| {
        let field: Option<&str> = match filter.name.as_str() {
            "AccountId" => Some(event.account_id.as_str()),
            "ResourceType" => Some(event.resource_type.as_str()),
            "EventType" => Some(event.event_type.as_str()),
            "EventStatus" => Some(event.event_status.as_str()),
```

The crate's OWN rule the accepted request breaks — `filters_violation`'s doc states the outcome a condition nothing can satisfy must not get —
services/.../src/state.rs:649
```
/// Why this listing's `filters` cannot narrow it, if they cannot: one of them
/// carries no value to select on.
...
/// every filter evaluator of this service requires ALL of the request's filters
/// to hold ([`rule_matches_filters`], [`event_matches_filters`],
/// [`recommended_action_matches_filters`]), so a filter carrying no value states
/// a condition nothing can satisfy: the request is refused under the floor every
/// caller-supplied value list of this service is held to
/// ([`empty_value_list_violation`]) rather than answered with an empty page that
/// reads as an authoritative result set.
```

The sibling that DOES refuse a prose-only value set admitted by the boundary —
services/.../src/ops/update_enrollment_configuration.rs:188
```
        // `status` documents which members of the enum a CALLER may send: "The
        // Pending and Failed options cannot be used to update the enrollment
        // status of an account. They are returned in the response". Both are
        // legal members of `EnrollmentStatus`, so the protocol boundary admits
        // them and the refusal belongs here. The rejection names the rejected
        // value and the statuses a caller may send instead.
        if !crate::state::SETTABLE_ENROLLMENT_STATUSES.contains(&input.status.as_str()) {
```

The crate comment whose premise the model contradicts —
services/.../src/state.rs:968
```
/// The model's remaining enum shapes — `EventStatus`, `EventType`, `StepStatus`,
/// `StepType`, `ResourceType`, `SavingsEstimationMode`,
/// `AutoEnrollMemberAccounts`, `OrganizationRuleModeSourceType`,
/// `OrganizationEnrollmentConfigurationStatus` —
/// appear in RESPONSES only, so no request can be refused against one
```

The value set is already generated into the crate and matches the member's prose exactly —
services/.../src/ops/types/shared_types.rs:249
```
pub const EVENT_STATUS_VALUES: &[&str] = &["Cancelled", "Complete", "Failed", "InProgress", "Ready", "RollbackComplete", "RollbackFailed", "RollbackInProgress", "RollbackReady"];
```

Test coverage: nothing pins the accepted behaviour. `tests/all/filter_value_lists.rs:43-58` drives every filtering operation, and every "selects nothing" row uses an IN-SET value that merely matches no record — `tests/all/filter_value_lists.rs:384` (`EventStatus` values `["Complete", "Failed"]`), `:390` (`AccountId` `["999999999999"]`), `:420` (`ResourceId` `["vol-9999"]`), `:348` (`Name` `["zeta-rule"]`). `tests/all/automation_events.rs:400` (`EventStatus-no-match`) uses `["Cancelled"]`, also in set. `tests/all/filter_name_refusals.rs:49` covers the filter NAME half only (`EVENT_FILTER_NAMES`). No test in the crate sends an `EventStatus` filter value outside `EVENT_STATUS_VALUES`.

## Steps to Reproduce

1. Start the emulator and open the enrollment gate: `UpdateEnrollmentConfiguration {"status":"Active"}`.
2. Create an automation event so the store is non-empty: `ListRecommendedActions {}`, then `StartAutomationEvent {"recommendedActionId":"<an id from step 2>"}`.
3. POST `/` with `X-Amz-Target: ComputeOptimizerAutomationService.ListAutomationEvents` and body `{"filters":[{"name":"EventStatus","values":["Done"]}]}` — `Done` matches `FilterValue`'s pattern but is not one of the nine values `AutomationEventFilter$values` documents for the `EventStatus` filter.
4. Observe HTTP 200 with `{"automationEvents": []}` and no `nextToken`, i.e. an authoritative-looking "no such events" answer, instead of a 400 `InvalidParameterValueException`.
5. Compare with `{"filters":[{"name":"EventStatus","values":[]}]}`, which the same handler refuses 400 through `filters_violation` for stating a condition nothing can satisfy — and with `UpdateEnrollmentConfiguration {"status":"Pending"}`, which is refused 400 for naming a value the member's prose excludes.
6. Repeat step 3 against `ListAutomationEventSummaries` with the same filter: same 200.
