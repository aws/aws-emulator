Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: aff844956fc3a746e6f61af85746cbd7333fe795

# Opt-in records NEXT_MAINTENANCE/IMMEDIATE even when the action's autoAppliedAfterDate/forcedApplyDate says such requests are ignored

_Filed: 2026-11-27T17:47:17Z_
_Operation: ApplyPendingMaintenanceAction | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `aff844956fc3a746e6f61af85746cbd7333fe795`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`PendingMaintenanceActionDetails` documents two dates that OVERRIDE an opt-in request: when `autoAppliedAfterDate` is set, a `NEXT_MAINTENANCE` `optInType` request is ignored; when `forcedApplyDate` is set, an `IMMEDIATE` request is ignored. The handler's `OptIn::apply_to` consults neither: it unconditionally stamps `optInStatus` and `currentApplyDate` on the detail for both opt-in types, so the response (and every later `GetPendingMaintenanceAction` / `ListPendingMaintenanceActions`) reports an opt-in the model says was ignored. For `NEXT_MAINTENANCE` the code even reads `autoAppliedAfterDate` and uses it as the effective date — the exact member whose presence is documented to make that request a no-op.

No behavioural impact today, because nothing in the crate ever puts an entry in `pending_maintenance_actions` (src/state.rs:123-132), so this branch is reached only once an action can be pending; the written rule is nevertheless the inverse of the modelled one.

## Evidence

Smithy, `PendingMaintenanceActionDetails` (`smithy_query(query="shape", shape="PendingMaintenanceActionDetails")`):
  `autoAppliedAfterDate`: "Displays the date of the maintenance window when the action is applied. ... If this date is specified, any <code>NEXT_MAINTENANCE</code> <code>optInType</code> requests are ignored."
  `forcedApplyDate`: "Displays the date when the maintenance action is automatically applied. ... If this date is specified, any <code>IMMEDIATE</code> <code>optInType</code> requests are ignored."

The handler applies both regardless — src/ops/apply_pending_maintenance_action.rs:158-184:
```
    fn apply_to(self, detail: &mut PendingMaintenanceActionDetails) {
        match self {
...
            Self::Immediate => {
                detail.opt_in_status = Some(state::OPT_IN_IMMEDIATE.to_string());
                detail.current_apply_date = Some(state::now_utc());
            }
...
            Self::NextMaintenance => {
                detail.opt_in_status = Some(state::OPT_IN_NEXT_MAINTENANCE.to_string());
                detail.current_apply_date = detail.auto_applied_after_date.clone();
            }
```
and the only caller writes it onto the stored detail — src/ops/apply_pending_maintenance_action.rs:297-299:
```
        opt_in.apply_to(detail);

        let resource_pending_maintenance_action = record.as_resource_pending_maintenance_action();
```
`forcedApplyDate` is never read anywhere in the crate (no occurrence outside the generated type in src/ops/types).

Test coverage: tests/all/maintenance_and_restore.rs:43-62 drives every `optInType` against clusters with nothing pending and only asserts the refusals (`APPLY_ON_REQUIRED`, pending-action-not-found), so no test pins the override behaviour either way.

## Steps to Reproduce

1. Give a cluster record a `PendingMaintenanceActionDetails` whose `action` is "ENGINE_UPDATE" and whose `autoAppliedAfterDate` is set (the member `ListPendingMaintenanceActions`/`GetPendingMaintenanceAction` report).
2. POST /pending-action with {"resourceArn": <clusterArn>, "applyAction": "ENGINE_UPDATE", "optInType": "NEXT_MAINTENANCE"}.
3. Observe the response detail carrying optInStatus=NEXT_MAINTENANCE and currentApplyDate=autoAppliedAfterDate, where the model states such a request is ignored (no opt-in recorded).
4. Repeat with an action carrying `forcedApplyDate` and optInType IMMEDIATE: optInStatus=IMMEDIATE and currentApplyDate=now are recorded, where the model states the request is ignored.
