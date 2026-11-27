Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 388bf0aeef5b87cdb8b8e5fe5948d049b57af517

# StartConfigurationChecks stores ConfigurationCheckIds entries without the enum-catalogue re-check every sibling handler applies before storing an enum member

_Filed: 2026-11-27T13:50:25Z_
_Operation: StartConfigurationChecks | Class: validation_

> _Note: this issue was identified against the code base at commit `388bf0aeef5b87cdb8b8e5fe5948d049b57af517`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every other handler in this crate that PERSISTS a value taken from a modelled enum member re-states the catalogue check in the handler, in the one place the crate words that refusal (`crate::state::value_within_catalogue`), and each one says in its own comment why: "so the handler never stores a <value> the API does not define". `StartConfigurationChecks` is the fourth such handler — it persists `ConfigurationCheckIds` entries as `StoredConfigurationCheckOperation::check_id` — and it is the only one that skips that check. It applies only a list-length bound (`configuration_checks_within_bounds`) and then hands the caller's raw strings to `configuration_checks_to_run`, which dedups them and returns them unchanged, straight into `record_configuration_check`.

The crate itself documents what an uncatalogued `check_id` reaching the store does, which is why this is a self-contradiction rather than a theoretical gap: `RegionState::settle` resolves the stored id against the catalogue and, when it is not there, ends the run in `ERROR` carrying the emulator-authored `uncatalogued_configuration_check_message`, and `StoredConfigurationCheckOperation::view` then reports that run with `ConfigurationCheckName`/`ConfigurationCheckDescription` absent. So the crate anticipates storing an id the catalogue does not hold — a state the three sibling handlers refuse to create for their own enum members with the `ValidationException` the operation models.

No reachable behaviour change is claimed: today the framework's boundary walk refuses a `ConfigurationCheckTypeList` element outside the enum before `handle` runs (see the fixed trace issue 20261022T232637Z-ssm-sap-startconfigurationchecks-enum-list-validation-message-reports-element-path, which is about the WORDING of exactly that 400). The defect is the divergence from the crate's own stated invariant: the handler that writes the longest-lived enum-derived record is the one that does not hold the line its siblings hold, and the `ERROR`-at-settle fallback is the only thing that would catch it.

## Evidence

The three siblings that DO re-check, each with the stated reason:

services/service-com-amazonaws-ssmsap-ssmsap/src/ops/register_application.rs:207-217
```
        // `ApplicationType` is an enum in the model, so the boundary validator has
        // already refused a value outside it; this re-states the refusal against the
        // model's own catalogue (`APPLICATION_TYPE_VALUES`) so the handler never stores
        // a type the API does not define. Worded in the ONE place this service refuses a
        // value outside a catalogue.
        crate::state::value_within_catalogue(
            "ApplicationType",
            APPLICATION_TYPE_VALUES,
            &input.application_type,
        )
        .map_err(RegisterApplicationErrorModeled::ValidationException)?;
```

services/service-com-amazonaws-ssmsap-ssmsap/src/ops/stop_application.rs:263-274
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
        .map_err(StopApplicationErrorModeled::ValidationException)?;
    }
```

services/service-com-amazonaws-ssmsap-ssmsap/src/ops/update_application_settings.rs:174-186
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
            .map_err(UpdateApplicationSettingsErrorModeled::ValidationException)?;
        }
```

The handler that does NOT — only a length bound, then the caller's strings are stored:

services/service-com-amazonaws-ssmsap-ssmsap/src/ops/start_configuration_checks.rs:205-212, 236-244
```
        // How many checks one request may name, re-stated in this crate before the
        // checks are resolved or any record is filed, so the work this request causes is
        // bounded by a check here rather than by the request body's size.
        crate::state::configuration_checks_within_bounds(input.configuration_check_ids.as_deref())
            .map_err(StartConfigurationChecksErrorModeled::ValidationException)?;

        let checks =
            crate::state::configuration_checks_to_run(input.configuration_check_ids.as_deref());
...
        let region_state = state.region_entry(&region);
        let mut started = Vec::with_capacity(checks.len());
        for check in &checks {
            started.push(
                region_state
                    .record_configuration_check(&application_id, check, now)
                    .view(),
            );
        }
```
(no `value_within_catalogue` / `CONFIGURATION_CHECK_TYPE_VALUES` call anywhere in this file)

services/service-com-amazonaws-ssmsap-ssmsap/src/state/configuration_checks.rs:1044-1059 — the resolver passes the caller's strings straight through for a non-empty list:
```
pub fn configuration_checks_to_run(requested: Option<&[String]>) -> Vec<String> {
    let named = requested.unwrap_or(&[]);
    if named.is_empty() {
        return CONFIGURATION_CHECK_TYPE_VALUES
            .iter()
            .map(|check| (*check).to_string())
            .collect();
    }
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut checks: Vec<String> = Vec::new();
    for check in named.iter().take(MAX_CONFIGURATION_CHECKS_PER_REQUEST) {
        if seen.insert(check.as_str()) {
            checks.push(check.clone());
        }
    }
    checks
}
```

services/service-com-amazonaws-ssmsap-ssmsap/src/state.rs:2667-2676 — the string is persisted verbatim as the record's `check_id`:
```
    pub fn record_configuration_check(
        &mut self,
        application_id: &str,
        check_id: &str,
        now: SystemTime,
    ) -> StoredConfigurationCheckOperation {
        let record = StoredConfigurationCheckOperation {
            id: new_operation_id(),
            application_id: application_id.to_string(),
            check_id: check_id.to_string(),
```

The crate's own fallback for a stored id the catalogue does not hold — proof the state is anticipated rather than impossible:

services/service-com-amazonaws-ssmsap-ssmsap/src/state.rs:2047-2051
```
            let evaluated = match self.applications.get(&application_id) {
                Some(application) => configuration_check_entry(&check_id)
                    .map(|entry| entry.evaluate(application, arn_region(&application.arn)))
                    .ok_or_else(|| uncatalogued_configuration_check_message(&check_id)),
```
services/service-com-amazonaws-ssmsap-ssmsap/src/state.rs:3698 `pub fn uncatalogued_configuration_check_message(check_id: &str) -> String`

services/service-com-amazonaws-ssmsap-ssmsap/src/state/configuration_checks.rs:938-954 — the projection drops the catalogue prose for such a record:
```
    /// `ConfigurationCheckName` / `ConfigurationCheckDescription` are the catalogue's
    /// prose for the check this run performs ([`configuration_check_entry`]) ... a
    /// check the catalogue does not hold is reported without them rather than with
    /// invented ones.
...
            configuration_check_name: catalogued.map(|entry| entry.name.to_string()),
```

The model's catalogue this check would be stated against: `smithy_query(query="shape", shape="ConfigurationCheckType")` -> `"values": ["SAP_CHECK_01", "SAP_CHECK_02", "SAP_CHECK_03"]`, mirrored in the crate as services/service-com-amazonaws-ssmsap-ssmsap/src/ops/types/shared_types.rs:153 `pub const CONFIGURATION_CHECK_TYPE_VALUES: &[&str] = &["SAP_CHECK_01", "SAP_CHECK_02", "SAP_CHECK_03"];`

Test coverage: tests/all/configuration_checks.rs, tests/all/configuration_check_catalogue.rs and tests/all/configuration_checks_in_flight.rs exercise starts, the catalogue and the in-flight conflict, but no test in tests/all pins a `ConfigurationCheckIds` entry outside the catalogue reaching the handler (the only coverage of that input is the boundary-validator wording recorded in the fixed issue 20261022T232637Z-ssm-sap-startconfigurationchecks-enum-list-validation-message-reports-element-path), so nothing pins the current omission with a stated reason.

## Steps to Reproduce

1. Open services/service-com-amazonaws-ssmsap-ssmsap/src/ops/register_application.rs and read lines 207-217, then src/ops/stop_application.rs lines 263-274, then src/ops/update_application_settings.rs lines 174-186: each handler that persists a modelled enum value calls crate::state::value_within_catalogue first, with a comment stating the handler must never store a value the API does not define.
2. Open services/service-com-amazonaws-ssmsap-ssmsap/src/ops/start_configuration_checks.rs and grep the file for value_within_catalogue / CONFIGURATION_CHECK_TYPE_VALUES: there is no match. The only pre-write check on ConfigurationCheckIds is the list-length bound at lines 208-209.
3. Follow src/state/configuration_checks.rs:1044-1059 (configuration_checks_to_run) — a non-empty list is deduped and returned unchanged — into src/state.rs:2667-2676 (record_configuration_check), which stores that string verbatim as the record's check_id.
4. Read src/state.rs:2047-2051: when settle resolves a stored check_id the catalogue does not hold, the run ends in ERROR carrying uncatalogued_configuration_check_message (src/state.rs:3698), and src/state/configuration_checks.rs:944-954 then reports that run with ConfigurationCheckName/ConfigurationCheckDescription absent — i.e. the crate carries a fallback for exactly the state its three sibling handlers refuse to create.
