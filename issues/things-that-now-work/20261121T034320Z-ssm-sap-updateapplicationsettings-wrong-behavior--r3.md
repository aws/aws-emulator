Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: d7cb5d802aaf50f8ef4cbec9c9028bf6415c27df

# An accepted settings update leaves Application$LastUpdated untouched, unlike every sibling control call that files a pending change

_Filed: 2026-11-21T03:43:20Z_
_Operation: UpdateApplicationSettings | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d7cb5d802aaf50f8ef4cbec9c9028bf6415c27df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every other control call of this crate stamps `application.last_updated = now` at the instant it files its `PendingTransition`, so a client reading the application right after the call sees that the record changed. `UpdateApplicationSettings` files a `PendingTransition` carrying `PendingSettings` and never stamps `last_updated`: between the accepted update and the instant the change falls due (one `TRANSITION_SPAN`), the application reports the SAME `LastUpdated` it had before the call — and since an update moves neither `Status` nor `DiscoveryStatus` and the credentials/Backint/DatabaseArn only change at settle, nothing at all about the application is observable as changed by a call that answered 200 with an `OperationIds` record.

The stamp is only applied later, by `StoredApplication::settle`, which sets it to the pending change's due instant. So the same field is maintained by two rules in this crate: filed-at-call-time for start/stop/refresh, and due-instant-only for a settings update. The field is reported by `Application$LastUpdated`, `Component$LastUpdated` and `Database$LastUpdated`, all projected from this one value.

## Evidence

The three sibling control handlers stamp it when they file the pending change:
- services/service-com-amazonaws-ssmsap-ssmsap/src/ops/start_application.rs:199-209:
```
application.status = crate::state::APPLICATION_STATUS_STARTING.to_string();
application.last_updated = now;
application.pending = Some(crate::state::PendingTransition {
```
- services/service-com-amazonaws-ssmsap-ssmsap/src/ops/stop_application.rs:304-306:
```
application.status = crate::state::APPLICATION_STATUS_STOPPING.to_string();
application.last_updated = now;
application.pending = Some(crate::state::PendingTransition {
```
- services/service-com-amazonaws-ssmsap-ssmsap/src/ops/start_application_refresh.rs:203-205:
```
application.discovery_status = crate::state::DISCOVERY_STATUS_REGISTERING.to_string();
application.last_updated = now;
application.pending = Some(crate::state::PendingTransition {
```

The settings update files the same kind of pending change with no stamp — services/service-com-amazonaws-ssmsap-ssmsap/src/ops/update_application_settings.rs:299-306:
```
application.pending = Some(crate::state::PendingTransition {
    due_at: now + crate::state::TRANSITION_SPAN,
    change: crate::state::PendingChange::Settings(crate::state::PendingSettings {
        credentials,
        backint: input.backint,
        database_arn: input.database_arn,
    }),
});
```
(the whole handler body, update_application_settings.rs:142-327, contains no assignment to `last_updated`).

The only other writer is the clock — services/service-com-amazonaws-ssmsap-ssmsap/src/state.rs:887:
```
self.last_updated = pending.due_at.max(self.last_updated);
```
and the value is what every read reports (state.rs:1059 `Application`, state.rs:1276 `Component`, state.rs:1365 `Database`).

Test coverage: no test pins `LastUpdated` across an update — tests/all/update_application_settings.rs never reads it; the only assertions on the field are tests/all/application_lifecycle.rs:676-679 (it is an epoch timestamp) and :828-832 (it is stable once nothing is owed).

## Steps to Reproduce

1. POST /register-application for a HANA application and wait past TRANSITION_SPAN so nothing is owed.
2. POST /get-application and record `Application.LastUpdated` (call it T_reg).
3. POST /update-application-settings for that application with `CredentialsToAddOrUpdate` naming a new database credential; observe 200 with `OperationIds`.
4. Immediately POST /get-application: `LastUpdated` is still T_reg, and Status, DiscoveryStatus and the credentials are unchanged — the accepted update is invisible on the application.
5. For contrast, POST /start-application on a settled application and immediately POST /get-application: `LastUpdated` is the instant of the call (start_application.rs:200).
6. Wait past TRANSITION_SPAN and POST /get-application again: `LastUpdated` jumps to the due instant (state.rs:887).
