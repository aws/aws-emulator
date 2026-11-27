Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: f2747f3074d15005d71be0a4e1cef8db0d1c04a6

# UpdateJobExecution judges statusDetails only after the stored execution is read, so a request-only refusal is answered as a state refusal

_Filed: 2026-11-27T01:05:12Z_
_Operation: UpdateJobExecution | Class: error-response_

> _Note: this issue was identified against the code base at commit `f2747f3074d15005d71be0a4e1cef8db0d1c04a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Both operations that accept `statusDetails` share ONE checker (`crate::state::check_status_details`, re-exported from `service_common::iot_job_executions`), and the crate states — in `StartNextPendingJobExecution` — that this check is decided from the request ALONE and is therefore made "before anything is looked up". `UpdateJobExecution` runs the identical check only at the very bottom of its handler, inside `record.try_replace_status_details(...)`, AFTER the 404 lookup, after the `expectedVersion` 400 and after both 409 state refusals.

The result is that the same inadmissible map produces opposite answers from the two handlers in this one crate. A `POST /things/{thingName}/jobs/{jobId}` carrying a `statusDetails` pair the record can never hold is answered 404 `ResourceNotFoundException` when the named execution does not exist, and 409 `InvalidStateTransitionException` when the execution has already finished (including the `TIMED_OUT` the handler itself has just materialised) — in neither case is the malformed member ever reported to the caller. The same map on `PUT /things/{thingName}/jobs/$next` is refused 400 `InvalidRequestException` naming the offending pair even when the thing has nothing pending at all (where, without the pre-check, the handler would answer 200 with `execution: null`).

This is not a claim about AWS: it is the crate contradicting the rule its own sibling handler states for the same member and the same checker.

## Evidence

The sibling states the rule and applies it before any state is read — services/service-com-amazonaws-iotjobsdataplane-iotlaserthingjobmanagerexternalservice/src/ops/start_next_pending_job_execution.rs:205-218:

    // A `statusDetails` map is checked against the bounds a job execution's
    // details carry before anything is looked up: a supplied map REPLACES the
    // stored one, so the check is on the request alone and a map the record
    // cannot hold is refused without touching any execution.
    if let Err(reason) = crate::state::check_status_details(input.status_details.as_ref()) {
        return Err(
            StartNextPendingJobExecutionErrorModeled::InvalidRequestException( ... )

Note this runs BEFORE `let mut state = state.lock();` (line 220) and before `state.next_pending_index(...)` (line 235).

The same rule, same checker, applied last — services/service-com-amazonaws-iotjobsdataplane-iotlaserthingjobmanagerexternalservice/src/ops/update_job_execution.rs:382-392:

    // A map outside what the record admits is the REQUEST's own fault, decided from
    // the map it carries and nothing the execution holds, so this refusal reports
    // no execution state.
    if let Err(reason) = record.try_replace_status_details(input.status_details.as_ref()) {
        return Err(UpdateJobExecutionErrorModeled::InvalidRequestException(
            RefusalDocument::message_only(InvalidRequestException { message: Some(reason) }),

but the state refusals are decided first, at update_job_execution.rs:275-287 (404 `ResourceNotFoundException` for an unknown execution), :308-323 (`expectedVersion` 400) and :336-375 (the two 409 `InvalidStateTransitionException` paths). `try_replace_status_details` itself begins `check_status_details(details)?` (services/common/src/iot_job_executions.rs:673-677), i.e. exactly the function the sibling called up front.

Test coverage of the member on both operations exists but exercises each against a healthy, resolvable execution only, so no test pins the ordering: tests/all/status_details_modeled_constraints.rs:170-219 ("Asserted on BOTH operations that accept the member", seeded with `emulator.start_command_on(&thing, None)` then immediately polled/updated) and tests/all/status_details_refusal_names_one_offender.rs:146.

## Steps to Reproduce

1. Start an emulator with this service mounted and create a command execution for thing `laser-x` (StartCommandExecution) so the thing has one QUEUED execution.
2. Send `POST /things/laser-x/jobs/no-such-job` with body `{"status":"IN_PROGRESS","statusDetails":{"step":"<1025 characters>"}}` — the handler answers 404 ResourceNotFoundException (`Job no-such-job not found for laser-x`) and never reports the inadmissible `statusDetails` value.
3. Let the started execution reach a terminal status (e.g. UpdateJobExecution status=SUCCEEDED, or let its step timer pass), then send `POST /things/laser-x/jobs/<jobId>` with the same over-long `statusDetails` — the handler answers 409 InvalidStateTransitionException, again never reporting the member.
4. Send `PUT /things/laser-y/jobs/$next` (a thing with nothing pending) with the same `statusDetails` map — the sibling handler answers 400 InvalidRequestException naming the offending pair, the answer the crate's stated rule prescribes for a refusal 'decided from the request alone'.
