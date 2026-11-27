Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: f2747f3074d15005d71be0a4e1cef8db0d1c04a6

# The device-update write sequence is implemented twice, once in $next and once in UpdateJobExecution

_Filed: 2026-11-27T01:05:12Z_
_Operation: StartNextPendingJobExecution | Class: maintainability_

> _Note: this issue was identified against the code base at commit `f2747f3074d15005d71be0a4e1cef8db0d1c04a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Applying a device update to a stored execution is one rule — replace `statusDetails`, (re-)arm the step timer, stamp `lastUpdatedAt`, move to `IN_PROGRESS` through `record.start(now)` when the update starts the execution, and increment `versionNumber` — and both writing handlers spell that sequence out themselves on the same `JobExecutionRecord`. The two copies already differ only in the order of two independent calls and in how the status is chosen, so a later change to one (for example disarming a timer, or not bumping the version) silently leaves the other behind, and the crate has no single place that says what a device update does. Folding the sequence into one helper this crate calls from both handlers leaves every response and every existing test unchanged.

## Evidence

Copy 1 — services/service-com-amazonaws-iotjobsdataplane-iotlaserthingjobmanagerexternalservice/src/ops/start_next_pending_job_execution.rs:275-299:

    if starts_the_execution || carries_a_member {
        if let Err(reason) = record.try_replace_status_details(input.status_details.as_ref())
        { ... }
        record.start(now);
        record.arm_step_timer(input.step_timeout_in_minutes, now);
        record.last_updated_at = now;
        // Starting the execution, and applying a member the device sent, are
        // both device updates, so the version moves.
        record.version_number += 1;
    }

Copy 2 — services/service-com-amazonaws-iotjobsdataplane-iotlaserthingjobmanagerexternalservice/src/ops/update_job_execution.rs:385-403:

    if let Err(reason) = record.try_replace_status_details(input.status_details.as_ref()) { ... }
    record.status = input.status.clone();
    record.arm_step_timer(input.step_timeout_in_minutes, now);
    if record.status == STATUS_IN_PROGRESS {
        // The device reporting IN_PROGRESS is a start: it records `startedAt`
        // and arms the job's in-progress timer on the first one.
        record.start(now);
    }
    record.last_updated_at = now;
    // "Job execution versions are incremented each time they are updated by a
    // device" — `JobExecution$versionNumber`.
    record.version_number += 1;

Both sides are pinned by existing tests, which the fix must leave untouched: tests/all/job_execution_state_machine.rs:521-533 ("RULE: `StartNextPendingJobExecution` STARTS a queued execution (version +1, `startedAt` stamped) ..."), tests/all/start_next_applies_the_members_it_carries.rs:243-251 ("Applying a member is a device update, so `lastUpdatedAt` and `versionNumber` move with it") and tests/all/job_execution_lifecycle.rs:415 ("a device's progress through an execution is persisted").

## Steps to Reproduce

1. Open src/ops/start_next_pending_job_execution.rs and read lines 275-299.
2. Open src/ops/update_job_execution.rs and read lines 385-403 — the same replace-details / arm-step-timer / start / stamp / version++ sequence, written again.
3. Change one of them (e.g. add a timer disarm on a terminal status, or stop bumping the version) and observe that nothing forces the other copy to follow.
