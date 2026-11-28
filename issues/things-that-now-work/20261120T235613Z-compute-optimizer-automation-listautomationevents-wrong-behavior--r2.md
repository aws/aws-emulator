Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4cec1d0eb9aabebbf70530b8e376ceefd6f10029

# The event listings report a disassociated member account's events, while every other selection in the crate narrows by the per-call association

_Filed: 2026-11-20T23:56:13Z_
_Operation: ListAutomationEvents | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4cec1d0eb9aabebbf70530b8e376ceefd6f10029`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate states one rule for reaching into another account: a disassociation leaves the member's stored resources alone, and from then on the caller reaches a resource of that account ONLY by id — every SELECTION over a store is narrowed by the association as it stands at the time of the call (`may_initiate_in`, read per call).

`ListRecommendedActions` and `ListRecommendedActionSummaries` implement that: both enter the recommended-action store through `guard.retrievable_actions()`, which filters on `may_initiate_in`. The two preview listings do the same via `previewed_actions`.

`ListAutomationEvents` and `ListAutomationEventSummaries` are the two selections over the EVENT store, and neither applies that narrowing: they walk `guard.events.iter_in_order()` and filter only by the retrieval window and by the caller's `filters`. So an automation event that was started in a member account while it was associated keeps being reported — by id (which the crate permits), AND by the listing and by the summary rows (which the crate's own rule says the organization has stopped reaching).

The result is an observable self-contradiction in one and the same call sequence: after `DisassociateAccounts(MEMBER_A)`, `ListRecommendedActions` no longer reports MEMBER_A's analysed actions, `RollbackAutomationEvent` on MEMBER_A's event is refused 403 `AccessDeniedException` (the handler reads `may_initiate_in` per call), yet `ListAutomationEvents` still lists that very event and `ListAutomationEventSummaries` still counts it and sums its `estimatedMonthlySavings` into a row. The event store is explicitly multi-account — `AutomationEventFilterName` declares an `AccountId` filter and `StoredEvent` carries `account_id` — so this is a reachable state, not a hypothetical one.

The gap is in both event-selection handlers; `ListAutomationEvents` is the one named here, and `ListAutomationEventSummaries` shares the same line-for-line omission (it is the same selection, grouped).

## Evidence

The rule the crate states, in the helper the gate lives in — note the "by id" qualifier, which is what makes a non-by-id read different:
`src/state.rs:3565-3569`: "Judged per call, never taken from the stored resource: a disassociation removes the association and leaves the member's events and analysed actions exactly where they were ([`Self::disassociate_account`]), so a resource of that account stays reachable by id after the organization has stopped reaching into it."

The same rule spelled out for selections, in the sibling store's accessor:
`src/state.rs:3646-3653`: "Every SELECTION over the stored actions starts here — the two retrieval listings and the preview selection ([`previewed_actions`]), which is what a rule would act on — so no read path reports an action of an account the caller no longer reaches, and none of them can drift apart from the operations that automate those actions. Resolving ONE action by its id still goes to the whole store ([`RecommendedActionStore::get`]), which is what lets `StartAutomationEvent` refuse with the caller's standing instead of claiming the resource has gone away."

The sibling listing that applies it:
`src/ops/list_recommended_actions.rs:246-250`: `let served: Option<(Vec<RecommendedAction>, Option<String>)> = crate::state::resume_page(\n    guard\n        .retrievable_actions()\n        .filter(|action| crate::state::recommended_action_matches_filters(action, &filters))`

The event listing that does NOT — no `may_initiate_in` / association filter anywhere in the chain:
`src/ops/list_automation_events.rs:263-275`: `let served: Option<(Vec<AutomationEvent>, Option<String>)> = crate::state::resume_page(\n    guard\n        .events\n        .iter_in_order()\n        .filter(|event| { crate::state::within_window(event.created_timestamp, Some(window_start), input.end_time_exclusive) })\n        .filter(|event| crate::state::event_matches_filters(event, &filters))\n        .map(crate::state::StoredEvent::to_automation_event),`

And the sibling summary, same omission:
`src/ops/list_automation_event_summaries.rs:319-331`: `let grouped = crate::state::group_events_by_event_status(\n    guard\n        .events\n        .iter_in_order()\n        .filter(|event| { crate::state::within_window(...) })\n        .filter(|event| crate::state::event_matches_filters(event, &filters)),`

The mutation path on the SAME event that does read the association per call, so the listing and the mutation disagree about the same event:
`src/ops/rollback_automation_event.rs:277-279`: `if !guard.may_initiate_in(&account_id) {\n    return Err(access_denied());\n}`

Test coverage: `tests/all/disassociated_member_retrieval.rs:5-8` states the rule for the recommended-action store ("a caller selects the analysis output of its OWN account and of each member account whose association is in place at the time of the call, and of no other account — the association is read per call, never inferred from the action being stored") and drives all four action/preview selections through association, removal and re-association. No test drives `ListAutomationEvents` or `ListAutomationEventSummaries` through a disassociation: `tests/all/member_account_initiation_gate.rs:353-399` reaches a disassociated member's event only through `GetAutomationEvent` (by id, line 393: `let unchanged = get_event(&router, event_id, case).await;`), and `event_ids_in_account` (line 105) is only ever asserted on accounts that hold no event at all (lines 297-309). So nothing pins the listing behaviour either way.

## Steps to Reproduce

1. UpdateEnrollmentConfiguration {"status":"Active"} to open the enrollment gate for the calling account.
2. AssociateAccounts {"accountIds":["111111111111"]} — the member's analysed inventory is recorded.
3. ListRecommendedActions {"filters":[{"name":"AccountId","values":["111111111111"]}]} — note one of the member's recommendedActionIds.
4. StartAutomationEvent {"recommendedActionId":<that id>} — note the returned eventId; the event is stored with accountId 111111111111.
5. DisassociateAccounts {"accountIds":["111111111111"]}.
6. ListRecommendedActions {"filters":[{"name":"AccountId","values":["111111111111"]}]} — now empty: the selection was narrowed by the removed association.
7. RollbackAutomationEvent {"eventId":<the eventId>} — refused 403 AccessDeniedException: the caller may no longer initiate work in that account.
8. ListAutomationEvents {"filters":[{"name":"AccountId","values":["111111111111"]}]} — the event is STILL reported, although it belongs to an account the crate's own gate says the organization has stopped reaching, and the only reach the crate grants after a disassociation is by id.
9. ListAutomationEventSummaries {"filters":[{"name":"AccountId","values":["111111111111"]}]} — the same event is still counted in a summary row and its estimatedMonthlySavings still summed into the row's total.
