Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 0fd0c8a38859f6782ef39e69b19293525b727c9f

# StopApplication accepts an application whose registration FAILED, erasing the FAILED status and giving its databases a running DatabaseStatus

_Filed: 2026-11-20T17:48:03Z_
_Operation: StopApplication | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `0fd0c8a38859f6782ef39e69b19293525b727c9f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every other control call of this crate asks the application's own registration outcome before it acts: `StartApplication` refuses a FAILED registration, and `StartApplicationRefresh` / `StartConfigurationChecks` refuse one that has not been registered successfully. `StopApplication` asks nothing: `record_stop` resolves the application, passes it through `application_control_available` (which is satisfied because a settled failed registration has `pending: None`) and then overwrites `application.status` with `STOPPING`, owing `STOPPED`.

Two things in this crate's own model then break:

1. The `FAILED` status the registration settled on is destroyed — `GetApplication` reports `Status: STOPPED` beside `DiscoveryStatus: REGISTRATION_FAILED`, a pair no other path can produce, while `StartApplication` on the same record keeps refusing (its gate reads `discovery_status`, which the stop leaves alone). The two reads of the same application now contradict each other.
2. `StoredDbms::status_for` decides `Database$Status` from the APPLICATION status, and its documented rule is that nothing is known about the databases of an application whose "discovery never succeeded". Before the stop a failed application's database reports `UNKNOWN` (status `FAILED` falls through to the `_` arm); after the stop the status is `STOPPED`, which is in the matched set, so `GetDatabase` reports the DBMS status — `RUNNING`, since a stop that names no `StopConnectedEntity` leaves the DBMS up — for a database of an application the service never discovered anything about.

The crate itself classifies `StopApplication` among the calls that move the application's own `Status` (`ApplicationControl::Stop => ControlRefusal::Transition("stopped")`), which is exactly the class `failed_registration_message` is documented for.

## Evidence

The documented rule, `services/service-com-amazonaws-ssmsap-ssmsap/src/state.rs:745-760`:
```
    /// The `DatabaseStatus` a database of an application whose `ApplicationStatus` is
    /// `application_status` reports.
    ///
    /// A database lives in the connected DBMS, so the status it reports is that DBMS's —
    /// but only from a status in which the service has established what the application
    /// runs at all. For any other application status (its registration is still under way,
    /// or discovery never succeeded) nothing is known about its databases, which is what
    /// the model's `UNKNOWN` states.
    pub fn status_for(&self, application_status: &str) -> &str {
        match application_status {
            APPLICATION_STATUS_ACTIVATED
            | APPLICATION_STATUS_STARTING
            | APPLICATION_STATUS_STOPPING
            | APPLICATION_STATUS_STOPPED => self.status.as_str(),
            _ => DATABASE_STATUS_UNKNOWN,
        }
    }
```
and it is what `Database$Status` is projected from — `src/state.rs:1361`: `status: Some(self.dbms.status_for(&self.status).to_string()),`

The sibling that DOES gate on the registration outcome, `src/ops/start_application.rs:172-184`:
```
        // A registration that FAILED leaves nothing to start: the application was never
        // discovered, so there is no SAP system behind it and the caller has to register
        // it again rather than wait. ...
        if application.registration_failed() {
            return Err(StartApplicationErrorModeled::ValidationException(
                crate::state::validation(crate::state::failed_registration_message(
```

The handler that skips it, `src/ops/stop_application.rs:287-317` (no `registration_failed()` / `registered_successfully()` check anywhere in the file — `grep registration_failed services/service-com-amazonaws-ssmsap-ssmsap/src/ops/` matches only `start_application.rs`):
```
    crate::state::application_control_available(
        &application,
        crate::state::ApplicationControl::Stop,
    )
    .map_err(StopApplicationErrorModeled::ConflictException)?;
    ...
    application.status = crate::state::APPLICATION_STATUS_STOPPING.to_string();
```
and the gate it relies on only exempts/decides on in-flight work — `src/state.rs:3760-3767`: `if application.pending.is_none() { return Ok(()); }` — which a settled failed registration satisfies.

The crate's own classification of this call, `src/state.rs:3720` and `src/state.rs:3732-3735`:
```
            Self::Stop => ControlRefusal::Transition("stopped"),
...
    /// A call that MOVES the application's own `Status` names the status it is in and the
    /// action it could not take, ...
    Transition(&'static str),
```
beside `src/state.rs:3806-3807`:
```
/// The message a call that moves an application's own status reports when the
/// registration of the application it names FAILED.
```

Test coverage: `tests/all/registration_outcome.rs:351-374` pins `StartApplication`'s refusal of a FAILED registration, and `tests/all/registration_outcome.rs:544-637` pins that a stop is served while a registration is still RUNNING (`a_stop_is_served_while_the_registration_is_still_running`) — a stop against an ALREADY-FAILED registration is driven by no test, so nothing pins the behaviour described here.

## Steps to Reproduce

1. POST /register-application with {"ApplicationId":"stop-failed","ApplicationType":"HANA","Instances":["i-0ffffffffffffffff"],"Sid":"HDB","SapInstanceNumber":"00","Credentials":[{"DatabaseName":"HDB","CredentialType":"ADMIN","SecretId":"sap/hdb/admin"}]} — an instance EC2 holds no record of, so the discovery cannot be carried out.
2. Wait out one TRANSITION_SPAN, then POST /get-application {"ApplicationId":"stop-failed"} — Status FAILED, DiscoveryStatus REGISTRATION_FAILED.
3. POST /get-database {"ApplicationId":"stop-failed","DatabaseId":"HDB"} — Database.Status is UNKNOWN, as `status_for` documents for an application whose discovery never succeeded.
4. POST /start-application {"ApplicationId":"stop-failed"} — 400 ValidationException "The registration of application stop-failed is failed."
5. POST /stop-application {"ApplicationId":"stop-failed"} — 200 with an OperationId (expected: the same refusal the start gives, since the application was never discovered).
6. Wait out one TRANSITION_SPAN, then POST /get-application again — Status is now STOPPED while DiscoveryStatus is still REGISTRATION_FAILED: the FAILED status is gone.
7. POST /get-database {"ApplicationId":"stop-failed","DatabaseId":"HDB"} — Database.Status is now RUNNING for an application whose discovery never succeeded, which `StoredDbms::status_for` documents as UNKNOWN.
