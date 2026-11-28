Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 1b14ccf8bc557e9f3ed20a9ce1fb318436d9b18d

# StopApplication serves an application whose registration FAILED, skipping the crate's own failed-registration refusal

_Filed: 2026-11-20T22:50:02Z_
_Operation: StopApplication | Class: validation_

> _Note: this issue was identified against the code base at commit `1b14ccf8bc557e9f3ed20a9ce1fb318436d9b18d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`StopApplication` moves the application's own `Status` (to `STOPPING`, then `STOPPED` on the clock) and files a `STOP_APPLICATION` record that settles `SUCCESS`, for an application whose registration has already SETTLED as failed (`Status`=FAILED / `DiscoveryStatus`=REGISTRATION_FAILED, i.e. nothing was ever discovered for it and the service manages no SAP system behind it).

The crate already holds the predicate and the refusal for exactly that settled condition — `StoredApplication::registration_failed()` and `failed_registration_message()` — and `failed_registration_message`'s own doc states the rule for the CLASS of calls it belongs to: "The message a call that moves an application's own status reports when the registration of the application it names FAILED." `StartApplication`, the sibling control call that moves the same `Status` field, calls it; `StopApplication` never consults `registration_failed()` at all.

The in-flight gate does not cover this either. `application_control_available` carves Stop out only for an application that is still `REGISTERING` (a pending change), which is a different, documented exception; a FAILED registration has no `pending`, so the gate returns `Ok(())` and the stop proceeds. `stop_application.rs`'s own module doc scopes its exception to the same narrow case ("the call is served for an application that is still `REGISTERING`") and says nothing about a registration that has settled as failed.

Side effect beyond the status: with `IncludeEc2InstanceShutdown: true` the handler then asks EC2 to stop `application.ec2_instance_ids()` for an application the service never discovered, and records those instance milestones on the operation record it answers with — a client reads a `SUCCESS` stop of an SAP system that was never registered successfully.

## Evidence

Crate rule (state.rs:3806-3816):
```
/// The message a call that moves an application's own status reports when the
/// registration of the application it names FAILED.
...
pub fn failed_registration_message(application_id: &str) -> String {
    format!("The registration of application {application_id} is failed.")
}
```
and the predicate it pairs with (state.rs:974-984): "Whether the registration of this application ENDED without completing ... this one is settled and will not become successful, so a call that needs a working application reports that the registration is failed rather than that work is in flight ([`failed_registration_message`])." — `pub fn registration_failed(&self) -> bool { self.discovery_status == DISCOVERY_STATUS_REGISTRATION_FAILED }`

Sibling that applies it, src/ops/start_application.rs:172-184:
```
// A registration that FAILED leaves nothing to start: the application was never
// discovered, so there is no SAP system behind it and the caller has to register
// it again rather than wait. ...
if application.registration_failed() {
    return Err(StartApplicationErrorModeled::ValidationException(
        crate::state::validation(crate::state::failed_registration_message(
            &input.application_id,
        )),
    )
    .into());
}
```

The handler that skips it, src/ops/stop_application.rs:273-317 — the only gate is the in-flight one, then the status is moved:
```
    crate::state::application_control_available(
        &application,
        crate::state::ApplicationControl::Stop,
    )
    .map_err(StopApplicationErrorModeled::ConflictException)?;
...
    application.status = crate::state::APPLICATION_STATUS_STOPPING.to_string();
```
No call to `registration_failed()` appears anywhere in `stop_application.rs` (grep for `registration_failed|failed_registration_message` across the crate matches only `start_application.rs:177/179`, `start_application_refresh.rs:180`, `start_configuration_checks.rs:188` and `state.rs`).

The documented exception is narrower than the behaviour. state.rs:3751-3767:
```
/// STOPPING is the exception, and the registration is what it is an exception to: a stop
/// does not wait for the registration that is still running ...
    if control == ApplicationControl::Stop
        && application.status == APPLICATION_STATUS_REGISTERING
    {
        return Ok(());
    }
```
and src/ops/stop_application.rs:24-29: "A registration still running does not hold a stop up ... so the call is served for an application that is still `REGISTERING`".

Test coverage: `tests/all/registration_outcome.rs:24-31` enumerates what the preconditions answer for a registration that did not settle successfully — `StartApplication` (400), `StartApplicationRefresh`/`StartConfigurationChecks` (409) — and the only Stop row is `a_stop_is_served_while_the_registration_is_still_running` (line 548), which registers and stops while the application is `REGISTERING`. No test exercises a stop against an application `register_failed` (line 267) has brought to FAILED/REGISTRATION_FAILED, so nothing pins the current behaviour for the settled-failed case.

## Steps to Reproduce

1. POST /register-application with {"ApplicationId":"stop-failed","ApplicationType":"HANA","Instances":["i-0ffffffffffffffff"],"Sid":"HDB","SapInstanceNumber":"00","Credentials":[{"DatabaseName":"SYSTEMDB","CredentialType":"ADMIN","SecretId":"secret"}]} — an instance EC2 holds no record of, so the discovery cannot be carried out.
2. Wait out `crate::state::TRANSITION_SPAN` and POST /get-application {"ApplicationId":"stop-failed"} — Application.Status = FAILED, DiscoveryStatus = REGISTRATION_FAILED (the state `tests/all/registration_outcome.rs::register_failed` sets up).
3. POST /start-application {"ApplicationId":"stop-failed"} — refused 400 ValidationException "The registration of application stop-failed is failed." (start_application.rs:177).
4. POST /stop-application {"ApplicationId":"stop-failed","IncludeEc2InstanceShutdown":true} — returns 200 with an OperationId instead of applying the same settled-failed refusal the sibling applies.
5. POST /get-application again: Status moves STOPPING then STOPPED while DiscoveryStatus stays REGISTRATION_FAILED; POST /get-operation with the returned id reports a SUCCESS STOP_APPLICATION record, and /list-operation-events reports an EC2 instance milestone for an application the service never discovered.
