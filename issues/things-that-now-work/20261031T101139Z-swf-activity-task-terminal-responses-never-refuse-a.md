Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: swf.concurrent-activity-task-terminal-response-race swf.activity-heartbeat-timeout-triggers-timeout-event swf.decision-token-used-for-activity-response-rejected swf.respond-activity-completed-after-task-timeout-rejected swf.concurrent-activity-pollers-no-duplicate-delivery
Issue-Shape-Id: com.amazonaws.swf#SimpleWorkflowService
Issue-Key: swf.concurrent-activity-task-terminal-response-race|run#29|AssertionError|assert len(successes) == 1, f"expected exactly one terminal response to win, got {outcomes}"
Issue-Key: swf.activity-heartbeat-timeout-triggers-timeout-event|run#17|AssertionError|assert code == "UnknownResourceFault", f"expected UnknownResourceFault, got {code!r}"
Issue-Key: swf.decision-token-used-for-activity-response-rejected|run#19|AssertionError|assert code == "UnknownResourceFault", f"expected UnknownResourceFault, got {code!r}"
Issue-Key: swf.respond-activity-completed-after-task-timeout-rejected|run#17|AssertionError|assert code == "UnknownResourceFault", f"expected UnknownResourceFault, got {code!r}"
Issue-Key: swf.concurrent-activity-pollers-no-duplicate-delivery|run#15|Timeout|results = list(pool.map(_poll, range(n_pollers)))
Issue-Commit: 3227a483a6739d1d495ce0f6668dabbc01e8fd5a

# swf: activity-task terminal responses never refuse a closed/stale token (RespondActivityTaskFailed still a 501 stub; RespondActivityTaskCompleted answers 501 where AWS answers UnknownResourceFault), and a concurrent PollForActivityTask race never completes

> _Note: this issue was identified against the code base at commit `3227a483a6739d1d495ce0f6668dabbc01e8fd5a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RespondActivityTaskCompleted

## Description
Three distinct root causes in the SWF activity-task data plane, all grounded in failing behavioral checks of this suite. They are numbered below; each must be closed before this issue is submitted.

Related open issues that do NOT cover these causes (named here only so a fixer can see the boundary): 20261029T184844Z-swf-six-fidelity-divergences-cancel-request-never-schedules.md (cancel-request / CountPending* / four wrong error classes), 20261029T230557Z-swf-error-responses-carry-the-wrong-status-code.md (error envelope status/code/message shape), 20261029T230557Z-swf-workflow-history-visibility-and-tag-responses-omit.md. None of them carries the replay keys listed here. The scaffold-stub issue auto-filed this run for `RespondActivityTaskCompleted` names the operation as unimplemented; Cause 2 below records the REFUSAL contract (code + exact message) that the three checks listed under it assert, which that stub issue does not state.

Cause 1 of 3: RespondActivityTaskFailed is an unimplemented scaffold stub, so no terminal response for an activity task can be recorded or refused.
AWS: Smithy (com.amazonaws.swf) models RespondActivityTaskFailed with input taskToken (@required, @length 1..1024), reason (0..256), details (0..32768), an empty (Unit) output on HTTP 200, and errors OperationNotPermittedFault (HTTP 400) and UnknownResourceFault (HTTP 400) — "Returned when the named resource cannot be found with in the scope of this operation". The operation doc: "A task is closed after it has been specified in a call to RespondActivityTaskCompleted, RespondActivityTaskCanceled, RespondActivityTaskFailed, or the task has timed out." Therefore exactly ONE terminal response per activity task token is accepted (200, empty body, an ActivityTaskFailed/ActivityTaskCompleted event appended to history), and a second, differing terminal response for the SAME, now-closed token is refused with UnknownResourceFault, HTTP 400, message "Unknown activity, scheduledEventId = <id>" — fixed text "Unknown activity, scheduledEventId = " plus the scheduledEventId of that activity task taken from the emulator's own history (recorded behavior; the model does not spell the message out).
Emulator: both racing calls are rejected. The check reports outcomes {'completed': ('error', 'NotImplemented'), 'failed': ('error', 'NotImplemented')} — i.e. HTTP 501 with error code NotImplemented from both RespondActivityTaskCompleted and RespondActivityTaskFailed, so neither terminal response wins and no outcome is recorded.
Why: a worker can never report activity failure at all; and the at-most-one-terminal-response invariant a client relies on (losing racer sees a 400 UnknownResourceFault and treats the task as already closed) is replaced by a 501 that botocore surfaces as a retryable/unknown server fault, so both workers believe the service is broken and the workflow never advances.
Fix: implement handle() in services/service-com-amazonaws-swf-simpleworkflowservice/src/ops/respond_activity_task_failed.rs (the SCAFFOLDED marker at its head is still present, and the body returns crate::scaffolded::Scaffolded::not_implemented) alongside the closed-token check of Cause 2, resolving the token against the stored history and appending ActivityTaskFailed only for a token whose activity task is still open.
Traces: swf.concurrent-activity-task-terminal-response-race

Cause 2 of 3: a stale, timed-out or wrong-kind activity task token is not refused with UnknownResourceFault — RespondActivityTaskCompleted answers 501 NotImplemented instead.
AWS: Smithy models RespondActivityTaskCompleted with taskToken (@required, @length 1..1024) and result (0..32768), Unit output, errors OperationNotPermittedFault (400) and UnknownResourceFault (400). A token whose activity task is no longer open names no resource, so the call is refused with HTTP 400, code UnknownResourceFault, message "Unknown activity, scheduledEventId = <scheduledEventId>" — fixed text "Unknown activity, scheduledEventId = " plus the scheduledEventId that the run's own ActivityTaskTimedOut/close event carries (recorded behavior for the message text; the model states only the error class). This covers all three token classes the checks exercise: a token whose task timed out on HEARTBEAT, a token whose task timed out on START_TO_CLOSE, and a DECISION task token passed to an activity response. History must not be mutated by the refused call (no ActivityTaskCompleted appears), and a subsequent healthy activity task on the same execution must still complete with 200.
Emulator: every one of these calls comes back as HTTP 501 / code NotImplemented (checks assert `expected UnknownResourceFault, got 'NotImplemented'`). Note the server-side timeout bookkeeping itself is correct — the checks got past their ActivityTaskTimedOut assertions with timeoutType HEARTBEAT and START_TO_CLOSE — so the divergence is purely the missing refusal (and the missing accept path).
Why: a worker that wakes up after its task timed out gets a 501 instead of a 400 UnknownResourceFault, so it cannot distinguish "my task is gone, drop the result" from "the service is unavailable, retry"; SDK retry logic then hammers the operation, and an idempotent-looking late completion is never positively rejected.
Fix: implement handle() in services/service-com-amazonaws-swf-simpleworkflowservice/src/ops/respond_activity_task_completed.rs — resolve the token to (domain, run, startedEventId) as crate::state::activity_task_token builds it, refuse with UnknownResourceFault + the message above when the addressed activity task is not open (timed out, already closed, or the token is a decision-task token), and otherwise append ActivityTaskCompleted and return the empty 200 body.
Traces: swf.activity-heartbeat-timeout-triggers-timeout-event, swf.decision-token-used-for-activity-response-rejected, swf.respond-activity-completed-after-task-timeout-rejected

Cause 3 of 3: concurrent PollForActivityTask callers on one task list never all return — the non-winning long polls are held for the full 60-second window, so a 4-poller dispatch race cannot complete.
AWS: recorded behavior only. PollForActivityTask is modelled with required domain + taskList and returns ActivityTask (taskToken @required @length 1..1024, or the documented EMPTY result where taskToken is an empty string). The doc gives 60 seconds as the MAXIMUM hold ("the service holds the HTTP connection open and responds as soon as a task becomes available. The maximum time the service holds on to the request before responding is 60 seconds"). The recording shows four simultaneous pollers on one task list holding exactly one scheduled activity task settle well inside the check's 60s budget: exactly one poller receives the task (activityId of the scheduled task) and the others come back with an empty taskToken promptly, after which CountPendingActivityTasks reports 0 pending for that list.
Emulator: the check never finishes — "Check timed out after 60.0s". The handler holds every poll that finds no waiting task until the full window elapses (services/service-com-amazonaws-swf-simpleworkflowservice/src/ops/poll_for_activity_task.rs loops until `waiting_until = now + crate::state::POLL_TIMEOUT`, with POLL_TIMEOUT = 60s in src/state.rs), so the three non-winning pollers cannot answer before the caller's own budget expires. Hypothesis: either the non-winning polls are simply held the whole maximum where AWS answers them as soon as the single task has been dispatched, or no poller is handed the task at all and all four sit out the window; the replay cannot distinguish these, so the fix must confirm which by re-replaying the check.
Why: any worker pool that fans out N pollers against one task list stalls for the full poll window per idle poller, and a client measuring dispatch dedup (or just draining a task list with a pool) hangs instead of returning; a 60s hard hold also pushes callers past botocore's default 60s read timeout, turning a normal empty poll into a ReadTimeoutError and a retry storm.
Fix: the long-poll wait in PollForActivityTask's handler (and the shared poll bookkeeping in src/state.rs it uses) — answer a poll as soon as the task list's state settles rather than parking it for the whole POLL_TIMEOUT, and make sure the single waiting task is handed to exactly one of the simultaneous pollers.
Traces: swf.concurrent-activity-pollers-no-duplicate-delivery

## Evidence
Suite: 39/66 passed (59.1%); swf 39/65 with 26 failures.

Cause 1 — [mismatch] swf.concurrent-activity-task-terminal-response-race:
  check_swf_concurrent_activity_task_terminal_response_race.py:118
  AssertionError: expected exactly one terminal response to win, got {'completed': ('error', 'NotImplemented'), 'failed': ('error', 'NotImplemented')}
  Expected: one of the two racing terminal responses returns HTTP 200 with an empty body and records its event; the other is refused HTTP 400 UnknownResourceFault, message "Unknown activity, scheduledEventId = <id>".
  Actual: HTTP 501, Error.Code "NotImplemented" from BOTH RespondActivityTaskCompleted and RespondActivityTaskFailed. respond_activity_task_failed.rs:189 returns Scaffolded::not_implemented("RespondActivityTaskFailed") and the file still carries the "//! SCAFFOLDED, NOT IMPLEMENTED" marker at line 4.

Cause 2 — three checks, same assertion:
  [mismatch] swf.activity-heartbeat-timeout-triggers-timeout-event, line 71:
    AssertionError: expected UnknownResourceFault, got 'NotImplemented'
    (the preceding assertion — ActivityTaskTimedOut with timeoutType 'HEARTBEAT' — passed)
  [mismatch] swf.decision-token-used-for-activity-response-rejected, line 58:
    AssertionError: expected UnknownResourceFault, got 'NotImplemented'
  [mismatch] swf.respond-activity-completed-after-task-timeout-rejected, line 69:
    AssertionError: expected UnknownResourceFault, got 'NotImplemented'
    (the preceding assertion — ActivityTaskTimedOut with timeoutType 'START_TO_CLOSE' — passed)
  Expected per check: Error.Code "UnknownResourceFault" on HTTP 400 and Error.Message exactly f"Unknown activity, scheduledEventId = {scheduled_id}", where scheduled_id is the scheduledEventId on the run's own ActivityTaskTimedOut event; no ActivityTaskCompleted appended; a later healthy task still completes.
  Actual: HTTP 501 / Error.Code "NotImplemented".

Cause 3 — [mismatch] swf.concurrent-activity-pollers-no-duplicate-delivery:
  Error: Check timed out after 60.0s (no assertion reached).
  Expected: 4 simultaneous PollForActivityTask calls on one task list with one scheduled task -> exactly 1 response with a non-empty taskToken and activityId 'act-1', the other 3 with an empty taskToken, then CountPendingActivityTasks(count)==0, all within 60s.
  Actual: the call set never completes inside 60s. poll_for_activity_task.rs:239-254 parks a task-less poll until now+POLL_TIMEOUT; state.rs:169 POLL_TIMEOUT = Duration::from_secs(60).

## Steps to Reproduce
1. Build and boot the emulator from this checkout and point a botocore swf client at it.
2. Cause 2: register a domain, a workflow type and an activity type on one task list; StartWorkflowExecution; PollForDecisionTask; RespondDecisionTaskCompleted with a ScheduleActivityTask decision (heartbeatTimeout=6 or startToCloseTimeout=6); PollForActivityTask to get the token; never heartbeat/respond and poll GetWorkflowExecutionHistory until ActivityTaskTimedOut appears; then call RespondActivityTaskCompleted(taskToken=<that stale token>, result="too-late"). Observe HTTP 501 / NotImplemented instead of HTTP 400 UnknownResourceFault with message "Unknown activity, scheduledEventId = <scheduledEventId from the ActivityTaskTimedOut event>". Same with a decision-task token passed to RespondActivityTaskCompleted.
3. Cause 1: schedule and start one activity task, then fire RespondActivityTaskCompleted and RespondActivityTaskFailed concurrently for the SAME taskToken. Observe both answer HTTP 501 / NotImplemented; expected is one 200 and one 400 UnknownResourceFault.
4. Cause 3: schedule exactly one activity task on a task list, wait until CountPendingActivityTasks reports >=1, then issue 4 simultaneous PollForActivityTask calls on that list from a thread pool. Observe the call set does not settle within 60s (non-winning polls are held the whole window) instead of one token + three empty results returning promptly.
5. Replay: swf.concurrent-activity-task-terminal-response-race, swf.activity-heartbeat-timeout-triggers-timeout-event, swf.decision-token-used-for-activity-response-rejected, swf.respond-activity-completed-after-task-timeout-rejected, swf.concurrent-activity-pollers-no-duplicate-delivery.
