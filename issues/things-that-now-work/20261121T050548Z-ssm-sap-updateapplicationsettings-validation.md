Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: ae01095c6362be2881945aa054bc93910bab485e

# UpdateApplicationSettings skips the registered-successfully precondition its siblings apply, so a FAILED registration accepts settings changes

_Filed: 2026-11-21T05:05:48Z_
_Operation: UpdateApplicationSettings | Class: validation_

> _Note: this issue was identified against the code base at commit `ae01095c6362be2881945aa054bc93910bab485e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`StoredApplication::registered_successfully` is documented as the precondition EVERY call that acts on the managed system has, and the crate gives it one construction site (`not_registered_successfully`) so every such call refuses alike. Two sibling control handlers apply it (`StartApplicationRefresh`, `StartConfigurationChecks`) and a third refuses the settled-failed case in its own words (`StartApplication`); `application_control_available` documents the single exception, which is `StopApplication` during `REGISTERING`.

`UpdateApplicationSettings` applies neither check. It resolves the application, applies only the in-flight conflict gate and the required-credentials rule, and then accepts the update. For an application whose registration FAILED — nothing was discovered, so the service manages no SAP system behind it — the call answers 200 with `"Your request to update the ... has been submitted."`, files an `UPDATE_APPLICATION_SETTINGS` record that settles to `SUCCESS`, and materialises the settings. Because an application's databases are DERIVED from its credentials and its SAP HANA connection, credentials added this way give a never-discovered application brand-new database resources that `ListDatabases`/`GetDatabase` then report and that `TagResource`/`PutResourcePermission` then resolve. Meanwhile `StartApplication` against that very application is refused 400 `The registration of application <id> is failed.`, so the crate both refuses and serves work on the same unusable registration depending on which handler is called.

## Evidence

The rule, in the crate's own words — `services/service-com-amazonaws-ssmsap-ssmsap/src/state.rs:962-972`:
```
    /// Whether the service's registration of this application COMPLETED successfully:
    /// its discovery ran and succeeded, so the service manages the SAP system the
    /// registration described.
    ///
    /// This is the precondition every call that acts on the managed system has: a
    /// registration that is still under way has discovered nothing yet and one that
    /// failed never will, and in both cases there is nothing for such a call to act on
    /// ([`not_registered_successfully`]).
    pub fn registered_successfully(&self) -> bool {
```
Siblings that apply it — `src/ops/start_configuration_checks.rs:188-193`:
```
        if !application.registered_successfully() {
            return Err(StartConfigurationChecksErrorModeled::ConflictException(
                crate::state::not_registered_successfully(),
            )
            .into());
        }
```
`src/ops/start_application_refresh.rs:180-185` is the same block, and `src/ops/start_application.rs:177-184` refuses the settled-failed case: `if application.registration_failed() { ... failed_registration_message(...) }`.

The handler that skips both — `src/ops/update_application_settings.rs:191-239`: after `applications.get(&input.application_id)` it calls only `crate::state::application_control_available(&application, crate::state::ApplicationControl::SettingsUpdate)` (line 213) and the required-credentials rule (line 229, `conditionally_required_members(&application.app_type)`); neither `registered_successfully()` nor `registration_failed()` appears anywhere in the file. The documented exception does not cover it — `src/state.rs:3751-3755`: "STOPPING is the exception, and the registration is what it is an exception to: a stop does not wait for the registration that is still running".

The derived-state consequence — `src/state.rs:1119-1162` (`databases` keys one record per credential `DatabaseName`) and `src/state.rs:872-884` (settle assigns `self.credentials = settings.credentials;`), so credentials accepted on a FAILED application mint database resources `RegionState::holds_resource`/`holds_database` then resolve (`src/state.rs:2249-2279`).

Test coverage: `tests/all/registration_outcome.rs:24-31` enumerates which calls are refused for a non-successful registration — `StartApplication`, `StartApplicationRefresh`, `StartConfigurationChecks`, with `StopApplication` served — and `tests/all/update_application_settings.rs` exercises the call only against successfully registered applications, so no test pins the accepted-on-FAILED behaviour.

## Steps to Reproduce

1. POST /register-application for `hdb-failed` naming an instance the EC2 owner holds no record of (`i-0ffffffffffffffff`), with the HANA members its type requires.
2. Wait out `crate::state::TRANSITION_SPAN`; POST /get-application confirms `FAILED`/`REGISTRATION_FAILED`.
3. POST /start-application {"ApplicationId":"hdb-failed"} — refused 400 `The registration of application hdb-failed is failed.`
4. POST /start-application-refresh {"ApplicationId":"hdb-failed"} — refused 409 `The specified application has not been registered successfully`.
5. POST /update-application-settings {"ApplicationId":"hdb-failed","CredentialsToAddOrUpdate":[{"DatabaseName":"TNT1","CredentialType":"ADMIN","SecretId":"s"}]} — answered 200 with a `Message` and an `OperationIds` entry, instead of the refusal the two calls above give for the same application state.
6. Wait out `TRANSITION_SPAN`, then POST /list-databases {"ApplicationId":"hdb-failed"} — a database derived from the accepted credential is now reported for an application the service discovered nothing for.
