Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: deadline.update-worker-schedule-progress-report-carrying-ended-at-rejected deadline.update-worker-schedule-progress-report-without-updated-at-rejected
Issue-Shape-Id: com.amazonaws.deadline#Deadline
Issue-Key: deadline.update-worker-schedule-progress-report-carrying-ended-at-rejected|_refusal#9|AssertionError|raise AssertionError(f"the request should have been refused, it returned {sorted(answer)!r}")
Issue-Key: deadline.update-worker-schedule-progress-report-without-updated-at-rejected|_refusal#9|AssertionError|raise AssertionError(f"the request should have been refused, it returned {sorted(answer)!r}")
Issue-Commit: f92d3cc1add58c930fe9ef1ae6c5aa7e04b710c1

# deadline UpdateWorkerSchedule: progress reports on updatedSessionActions are never validated — endedAt on an update action and a report missing updatedAt are both accepted (200) instead of refused with ValidationException

> _Note: this issue was identified against the code base at commit `f92d3cc1add58c930fe9ef1ae6c5aa7e04b710c1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateWorkerSchedule

## Description
Two net-new root causes in the deadline (Deadline Cloud) emulator's UpdateWorkerSchedule handler. Both concern the per-action validation of the request's `updatedSessionActions` map: the emulator applies every reported field unconditionally, so two refusals AWS always makes are missing. Both must be closed before submitting. Neither is covered by the open issue 20261127T092702Z-deadline-worker-scheduling-data-plane-ten-divergences-missing.md (its Cause 3 is the worker-status gate on UpdateWorkerSchedule and its Cause 10 is the logConfiguration.parameters member — neither is the updatedSessionActions field validation described here), nor by 20261125T140150Z-deadline-getqueueenvironment-404-names-the-resource-type-environment.md, nor by the AssumeFleetRoleForWorker / ListWorkers / GetTask / AssumeQueueRoleForRead scaffold-stub issues.

--- Cause 1 of 2: a progress report that carries endedAt is accepted instead of refused ---
AWS: Smithy UpdateWorkerSchedule (PATCH /2023-10-12/farms/{farmId}/fleets/{fleetId}/workers/{workerId}/schedule, host prefix scheduling.) takes `updatedSessionActions` (map of session-action id -> UpdatedSessionActionInfo {completedStatus, processExitCode, progressMessage, startedAt, endedAt, updatedAt, progressPercent, manifests}) and models ValidationException (HTTP 400) whose required members are `message` and `reason` (ValidationExceptionReason: UNKNOWN_OPERATION|CANNOT_PARSE|FIELD_VALIDATION_FAILED|OTHER). The model marks no member of UpdatedSessionActionInfo required, so the rule distinguishing an *update* (progress) report from a *completion* report is recorded behavior only: an entry WITHOUT `completedStatus` is an update action, and an update action may not carry `endedAt` — only a completion report (one carrying completedStatus, e.g. SUCCEEDED with processExitCode) may. Recorded behavior: an update entry carrying endedAt is refused 400 ValidationException with message "Failed to update session action: endedAt cannot be provided for update actions". That message is entirely fixed text — it names no id, value or timestamp — and the whole report is refused atomically: the named session action keeps the status and progress it had before the call (no status move to RUNNING, no progressPercent/progressMessage recorded). The same entry with endedAt removed is accepted and recorded, and a completion entry {completedStatus: SUCCEEDED, processExitCode, startedAt, endedAt, progressPercent} is accepted.
Emulator: HTTP 200 with the ordinary schedule body — the response carries ['assignedSessions', 'cancelSessionActions', 'updateIntervalSeconds'] and no error at all. services/service-com-amazonaws-deadline-deadline/src/ops/update_worker_schedule.rs validates only that every reported id belongs to a session of THIS worker (ResourceNotFoundException for an unknown id) and otherwise hands the whole map to crate::state::...::report_session_actions, which applies the reported fields as given; no entry is inspected for the endedAt/completedStatus combination.
Why: a worker agent that mistakenly stamps endedAt on a mid-session progress report has it silently accepted, so a still-running session action is recorded as ended. The negative branch of the agent's reporting contract is never exercised, and a client that relies on Deadline Cloud rejecting the malformed report (and on the action being left untouched) instead observes its own bad report become the recorded truth.
Fix: the UpdateWorkerSchedule handler's input validation (services/service-com-amazonaws-deadline-deadline/src/ops/update_worker_schedule.rs) — classify each updatedSessionActions entry as an update or a completion report and reject the whole request with the modeled ValidationException (reason FIELD_VALIDATION_FAILED) BEFORE calling report_session_actions, so nothing is half-applied.
Traces: deadline.update-worker-schedule-progress-report-carrying-ended-at-rejected.

--- Cause 2 of 2: a progress report without updatedAt is accepted instead of refused ---
AWS: same operation and same modeled ValidationException (400, required message + reason). Recorded behavior only for the rule: an update action (an updatedSessionActions entry with no completedStatus) MUST say when the report was taken — without the `updatedAt` member the request is refused 400 ValidationException with message "Failed to update session action: updatedAt is required for update actions". That message is entirely fixed text (the member name `updatedAt` and the phrase "update actions" are part of it; no request value appears in it). The refusal holds both before the action has started reporting and while it is already RUNNING, is atomic — the action's status and progressPercent are exactly what they were before the call — and the same entry WITH updatedAt is accepted and moves the action to RUNNING with the reported progressPercent.
Emulator: HTTP 200 with ['assignedSessions', 'cancelSessionActions', 'updateIntervalSeconds'] for an entry of only {startedAt, progressPercent}; the report is recorded.
Why: an agent can advance a session action's recorded progress with no timestamp, so the service's own ordering of progress reports (which depends on updatedAt) has no basis, and a client that expects undated reports to bounce sees them land instead.
Fix: the same per-entry validation in the UpdateWorkerSchedule handler (services/service-com-amazonaws-deadline-deadline/src/ops/update_worker_schedule.rs) — require updatedAt on every non-completion entry and raise the modeled ValidationException (reason FIELD_VALIDATION_FAILED) before any state is written.
Traces: deadline.update-worker-schedule-progress-report-without-updated-at-rejected.

## Evidence
Replay output (behavioral suite, service heading '### deadline'):

Cause 1 — [mismatch] deadline.update-worker-schedule-progress-report-carrying-ended-at-rejected:
  File "check_deadline_update_worker_schedule_progress_report_carrying_ended_at_rejected.py", line 70, in run
    refused = _refusal(lambda: dl.update_worker_schedule(
  AssertionError: the request should have been refused, it returned ['ResponseMetadata', 'assignedSessions', 'cancelSessionActions', 'updateIntervalSeconds']
The call was UpdateWorkerSchedule(farmId=<farm>, fleetId=<fleet>, workerId=<worker>, updatedSessionActions={<assignedActionId>: {startedAt: <t>, updatedAt: <t>, progressPercent: 40.0, progressMessage: 'rendering', endedAt: <t>}}) on an action in a live status (ASSIGNED/SCHEDULED/RUNNING/INTERRUPTING). Expected: HTTP 400, Error.Code 'ValidationException', Error.Message 'Failed to update session action: endedAt cannot be provided for update actions' (fixed text, no variable parts), and GetSessionAction unchanged across the refused call. Got: HTTP 200 and the schedule body above.

Cause 2 — [mismatch] deadline.update-worker-schedule-progress-report-without-updated-at-rejected:
  File "check_deadline_update_worker_schedule_progress_report_without_updated_at_rejected.py", line 72, in run
    first = _refusal(lambda: report(startedAt=began, progressPercent=25.0))
  AssertionError: the request should have been refused, it returned ['ResponseMetadata', 'assignedSessions', 'cancelSessionActions', 'updateIntervalSeconds']
The call was UpdateWorkerSchedule(..., updatedSessionActions={<assignedActionId>: {startedAt: <t>, progressPercent: 25.0}}). Expected: HTTP 400, Error.Code 'ValidationException', Error.Message 'Failed to update session action: updatedAt is required for update actions' (fixed text, no variable parts), with the action's status/progressPercent unchanged. Got: HTTP 200 and the schedule body above. The check then asserts the same refusal for an undated report on an already-RUNNING action (progressPercent=75.0 with the recorded 25.0 kept); that assertion was never reached because the first call already succeeded, so fix both occurrences.

Handler source: services/service-com-amazonaws-deadline-deadline/src/ops/update_worker_schedule.rs lines 199-216 — the only check on `input.updated_session_actions` is the unknown-session-action-id ResourceNotFoundException; the map is otherwise passed straight to crate::state::...::report_session_actions.

## Steps to Reproduce
Boot the emulator and replay the behavioral suite; the two checks in trace_ids fail. By hand (deadline client, scheduling endpoint):
1. CreateFarm, CreateFleet (maxWorkerCount >= 1, with a worker role), CreateQueue, CreateQueueFleetAssociation, CreateWorker, UpdateWorker(status='STARTED', capabilities=<host caps>).
2. CreateJob on the queue and poll UpdateWorkerSchedule until a session action is assigned; note <actionId> and confirm GetSessionAction reports a live status.
3. UpdateWorkerSchedule(farmId=<farm>, fleetId=<fleet>, workerId=<worker>, updatedSessionActions={<actionId>: {'startedAt': now, 'updatedAt': now, 'progressPercent': 40.0, 'progressMessage': 'rendering', 'endedAt': now}}) -> emulator answers 200 with the schedule; AWS answers 400 ValidationException 'Failed to update session action: endedAt cannot be provided for update actions' and leaves GetSessionAction(<actionId>) byte-for-byte unchanged (Cause 1).
4. UpdateWorkerSchedule(..., updatedSessionActions={<actionId>: {'startedAt': now, 'progressPercent': 25.0}}) -> emulator answers 200 and records the progress; AWS answers 400 ValidationException 'Failed to update session action: updatedAt is required for update actions' with the action untouched (Cause 2).
5. Send the same entry WITH updatedAt, confirm it is accepted and the action becomes RUNNING at the reported progressPercent, then repeat step 4 against the RUNNING action: AWS refuses it identically and keeps the previously recorded progress.
