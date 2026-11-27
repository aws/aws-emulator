Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 803b8f0256cf78557f71fb793e40c4a31d070d39

# ApplyPendingMaintenanceAction accepts any applyOn string and any applyAction string, though both members document a value domain this crate validates elsewhere

_Filed: 2026-11-27T12:53:45Z_
_Operation: ApplyPendingMaintenanceAction | Class: validation_

> _Note: this issue was identified against the code base at commit `803b8f0256cf78557f71fb793e40c4a31d070d39`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`OptIn::parse` enforces only the PRESENCE of `applyOn` for `optInType: APPLY_ON`; the string itself is taken verbatim (`Ok(Self::On(date.to_string()))`) and later copied straight into `currentApplyDate`, even though the member documents an exact format. Likewise `applyAction` is matched against the stored actions by raw string equality with no check against the documented action set, so `applyAction: "not-an-action"` and `applyAction: "ENGINE_UPDATE"` are indistinguishable at the request-rule stage.

This crate validates the documented format of every OTHER date/time member it accepts, in the same state module and from sibling handlers: `preferredMaintenanceWindow` is held to "Format: ddd:hh24:mi-ddd:hh24:mi" and normalised, and `preferredBackupWindow` to `hh24:mi-hh24:mi`, both refused with a `ValidationException` before anything is stored. The opt-in path is the one documented-format member left unchecked, so a request carrying a malformed `applyOn` is not refused for the rule it broke — it falls through to the unrelated "Pending maintenance action not found for ResourceArn ..." sentence, which tells the caller its ARN has nothing pending rather than that its date is not a date.

## Evidence

Smithy, `ApplyPendingMaintenanceActionInput$applyOn` (`smithy_query(query="doc", shape="ApplyPendingMaintenanceActionInput", member="applyOn")`):

> "A specific date to apply the pending maintenance action. Required if opt-in-type is `APPLY_ON`. Format: `yyyy/MM/dd HH:mm-yyyy/MM/dd HH:mm`"

Smithy, `ApplyPendingMaintenanceActionInput$applyAction`:

> "The pending maintenance action to apply to the resource. Valid actions are: `ENGINE_UPDATE` `ENGINE_UPGRADE` `SECURITY_UPDATE` `OS_UPDATE` `MASTER_USER_PASSWORD_UPDATE`"

The handler checks presence only, and stores/echoes the string as given:

`services/service-com-amazonaws-docdbelastic-chimeradblionfishservicelambda/src/ops/apply_pending_maintenance_action.rs:135-143`
```
    fn parse(opt_in_type: &str, apply_on: Option<&str>) -> Result<Self, ValidationException> {
        match opt_in_type {
            state::OPT_IN_APPLY_ON => match apply_on {
                Some(date) => Ok(Self::On(date.to_string())),
                None => Err(apply_on_required()),
            },
```
`services/service-com-amazonaws-docdbelastic-chimeradblionfishservicelambda/src/ops/apply_pending_maintenance_action.rs:161-164`
```
            Self::On(date) => {
                detail.opt_in_status = Some(state::OPT_IN_APPLY_ON.to_string());
                detail.current_apply_date = Some(date);
            }
```
and `applyAction` is compared raw, with no check against the documented set:
`.../apply_pending_maintenance_action.rs:285-288`
```
        let Some(detail) = record
            .pending_maintenance_actions
            .iter_mut()
            .find(|detail| detail.action == input.apply_action)
```

The sibling rule this one skips — the crate's own enforcement of a documented `Format:` on a window member, called from `CreateCluster` and `UpdateCluster`:

`services/service-com-amazonaws-docdbelastic-chimeradblionfishservicelambda/src/state.rs:1360-1386`
```
/// Refuse a `preferredMaintenanceWindow` outside the documented
/// `ddd:hh24:mi-ddd:hh24:mi` form ...
/// Source: `CreateClusterInput$preferredMaintenanceWindow` doc — "Format:
/// ddd:hh24:mi-ddd:hh24:mi … Valid days: Mon, Tue, Wed, Thu, Fri, Sat, Sun".
pub fn validate_maintenance_window(
```
`services/service-com-amazonaws-docdbelastic-chimeradblionfishservicelambda/src/ops/create_cluster.rs:236-246`
```
        let preferred_maintenance_window = match input.preferred_maintenance_window.as_deref() {
            Some(window) => Some(
                state::validate_maintenance_window("preferredMaintenanceWindow", window)
 ...
        if let Some(window) = input.preferred_backup_window.as_deref() {
            state::validate_backup_window("preferredBackupWindow", window)
```

Test coverage: `tests/all/maintenance_and_restore.rs:64-68` records the documented format as its fixture —
```
/// A date in the documented `applyOn` form.
/// Source: `ApplyPendingMaintenanceActionInput$applyOn` doc — "Format: yyyy/MM/dd
/// HH:mm-yyyy/MM/dd HH:mm"
const APPLY_ON_WINDOW: &str = "2027/03/14 02:00-2027/03/14 03:00";
```
— and its rows only cover the present/absent split (`.../maintenance_and_restore.rs:366-401`, asserting the fixed `APPLY_ON_REQUIRED` sentence); no row sends a malformed date or an action outside the documented set, so neither is pinned. By contrast `tests/all/cluster_lifecycle.rs:1110-1121` pins 400 for "a maintenance window that is free text" and "a backup window that is free text".

## Steps to Reproduce

1. Start the emulator and CreateCluster(clusterName='apply1', ...) to get a clusterArn.
2. POST /pending-action with {"resourceArn": <arn>, "applyAction": "ENGINE_UPDATE", "optInType": "APPLY_ON", "applyOn": "nightly"} — a value that is not the documented `yyyy/MM/dd HH:mm-yyyy/MM/dd HH:mm` form.
3. Observe the request passes OptIn::parse unchecked (apply_pending_maintenance_action.rs:137-140) and is answered with the unrelated "Pending maintenance action not found for ResourceArn <arn>" instead of a refusal naming the applyOn format rule, which is what CreateCluster answers for a free-text preferredMaintenanceWindow (cluster_lifecycle.rs:1110-1113).
4. Repeat with {"applyAction": "not-an-action", "optInType": "IMMEDIATE"} and observe the action string is never held to the documented action set either.
