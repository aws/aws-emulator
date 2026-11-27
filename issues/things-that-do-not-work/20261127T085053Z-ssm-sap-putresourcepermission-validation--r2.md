Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 91b993ae414910f3a059c9a9af3882a70940e0b6

# PutResourcePermission stores ActionType without the catalogue re-check every sibling handler applies to a stored enum member

_Filed: 2026-11-27T08:50:53Z_
_Operation: PutResourcePermission | Class: validation_

> _Note: this issue was identified against the code base at commit `91b993ae414910f3a059c9a9af3882a70940e0b6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`PutResourcePermission` persists the caller's `ActionType` as a map key in `RegionState::resource_permissions` and renders it into the policy document's `Action` (`permission_policy_action`), but it never passes that value through `crate::state::value_within_catalogue`, the single place this crate refuses a value outside a catalogue.

Every other handler of this crate that STORES a modelled enum member does re-check it, and each says why in the same words: `RegisterApplication` for `ApplicationType`, `UpdateApplicationSettings` for `BackintMode`, `StopApplication` for `StopConnectedEntity` — all three note that the boundary validator has already refused anything outside the enum and that the re-statement exists "so the handler never stores a [value] the API does not define". The generated catalogue for this member, `PERMISSION_ACTION_TYPE_VALUES`, already exists in `src/ops/types/shared_types.rs` and is referenced by nothing.

So the one handler that writes an enum value into long-lived state keyed by that value is the one path that skips a helper the crate already has, while its three siblings apply it. `PutResourcePermission` models `ValidationException`, the error the helper's refusal is lifted into everywhere else, so there is no error-channel obstacle.

## Evidence

The gap — services/service-com-amazonaws-ssmsap-ssmsap/src/ops/put_resource_permission.rs:174-213 performs only the `holds_database` resolution of `ResourceArn` and then stores the action type unchecked:
```
        // `ActionType`, `ResourceArn` and `SourceResourceArn` are all modelled
        // `@required` with their own constraints, so reaching here means each is
        // present and well formed.
...
        let outcome = state.region_entry(&region).grant_resource_permission(
            &input.resource_arn,
            &input.action_type,
            &input.source_resource_arn,
        );
```
and that value becomes a persisted key — services/service-com-amazonaws-ssmsap-ssmsap/src/state.rs:2448-2453:
```
        self.resource_permissions
            .entry(resource_arn.to_string())
            .or_default()
            .entry(action_type.to_string())
            .or_default()
            .insert(source_resource_arn.to_string());
```

The sibling handlers apply the check — services/service-com-amazonaws-ssmsap-ssmsap/src/ops/update_application_settings.rs:174-186:
```
        if let Some(backint) = input.backint.as_ref() {
            // `BackintMode` is an enum in the model, so the boundary validator has
            // already refused a value outside it; this re-states the refusal against the
            // model's own catalogue (`BACKINT_MODE_VALUES`) so the handler never stores a
            // mode the API does not define. Worded in the ONE place this service refuses
            // a value outside a catalogue.
            crate::state::value_within_catalogue(
                "BackintMode",
                BACKINT_MODE_VALUES,
                &backint.backint_mode,
            )
```
services/service-com-amazonaws-ssmsap-ssmsap/src/ops/stop_application.rs:258-269:
```
    if let Some(entity) = stop_connected_entity {
        // `StopConnectedEntity` is an enum in the model, so the boundary validator
        // has already refused anything outside it; this re-states the refusal
        // against the model's own catalogue, in the ONE place this service refuses a
        // value outside a catalogue.
        crate::state::value_within_catalogue(
            "StopConnectedEntity",
            CONNECTED_ENTITY_TYPE_VALUES,
            entity,
        )
```
services/service-com-amazonaws-ssmsap-ssmsap/src/ops/register_application.rs:212-217 does the same for `ApplicationType`.

The helper and the catalogue both already exist and are unused by this path — services/service-com-amazonaws-ssmsap-ssmsap/src/state.rs:3316-3330:
```
/// Refuse a member carrying a value outside the catalogue that member admits.
...
/// An enum member is also walked by the boundary validator from the model, so for one of
/// those this cannot normally fire; it is here so no handler stores a value the API does
/// not define, whatever reaches it.
pub fn value_within_catalogue(
```
services/service-com-amazonaws-ssmsap-ssmsap/src/ops/types/shared_types.rs:196-197:
```
/// The values `PermissionActionType` accepts on the wire.
pub const PERMISSION_ACTION_TYPE_VALUES: &[&str] = &["RESTORE"];
```
`grep -rn 'PERMISSION_ACTION_TYPE_VALUES' services/service-com-amazonaws-ssmsap-ssmsap/src` matches only that declaration.

Test coverage: tests/all/resource_permissions.rs and tests/all/resource_permission_reads.rs exercise grant/read/revoke with the modelled `RESTORE` value; no test pins the absence of the catalogue re-check, and adding it changes no response for a value the model admits.

## Steps to Reproduce

1. Open services/service-com-amazonaws-ssmsap-ssmsap/src/ops/put_resource_permission.rs and read the handler body (lines 155-226): the only refusal it raises is the `holds_database` check on `ResourceArn`; `input.action_type` is passed to `grant_resource_permission` with no validation.
2. Open src/ops/update_application_settings.rs:174-186, src/ops/stop_application.rs:258-269 and src/ops/register_application.rs:212-217: each stored enum member is re-checked with `crate::state::value_within_catalogue` against its generated `*_VALUES` catalogue, each with the comment that the re-statement exists so no handler stores a value the API does not define.
3. Run `grep -rn 'PERMISSION_ACTION_TYPE_VALUES' services/service-com-amazonaws-ssmsap-ssmsap/src`: the only hit is the declaration in src/ops/types/shared_types.rs:197 — the catalogue for the one enum member this service persists is never consulted.
