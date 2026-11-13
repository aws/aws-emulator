Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: swf.decision-token-used-for-activity-response-rejected
Issue-Shape-Id: com.amazonaws.swf#SimpleWorkflowService
Issue-Key: swf.decision-token-used-for-activity-response-rejected|run#26|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: 043b7f8056dcef57c0c9baaf75abb2dd363a8bd0

# swf RespondActivityTaskCompleted refuses a non-activity (decision) taskToken with scheduledEventId = 0 instead of the scheduledEventId the token addresses

> _Note: this issue was identified against the code base at commit `043b7f8056dcef57c0c9baaf75abb2dd363a8bd0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RespondActivityTaskCompleted

## Description
One net-new root cause in the SWF activity-task data plane, grounded in one failing behavioral check of this suite.

Cause 1 of 1: the `UnknownResourceFault` message RespondActivityTaskCompleted answers for a taskToken that does not address an open activity task reports `scheduledEventId = 0` whenever the token is not an ACTIVITY task token, instead of the scheduledEventId the token actually addresses.
Failing check: swf.decision-token-used-for-activity-response-rejected.

Boundary with already-open issues (named only so a fixer can see what this is NOT): 20261031T101139Z-swf-activity-task-terminal-responses-never-refuse-a.md, Cause 2, covers the same check's EARLIER failure — the operation answering HTTP 501 / `NotImplemented` where AWS answers `UnknownResourceFault`. That refusal now works: the check gets past `assert code == "UnknownResourceFault"` and fails one assertion later, on the message text, with a replay key that issue does not carry. This issue is only the message's variable part. 20261029T230557Z-swf-error-responses-carry-the-wrong-status-code.md covers the error ENVELOPE (status, code payload shape), not this message's content; 20261029T184844Z-swf-six-fidelity-divergences-cancel-request-never-schedules.md covers cancel-request / CountPending* / four other error classes. None covers this.

AWS: Smithy (com.amazonaws.swf#RespondActivityTaskCompleted) models input `taskToken` (TaskToken, @required, @length 1..1024) and `result` (Data, @length 0..32768), a Unit output on HTTP 200, and errors `OperationNotPermittedFault` (HTTP 400) and `UnknownResourceFault` (HTTP 400) — "Returned when the named resource cannot be found with in the scope of this operation (region or domain)." The message text is recorded behavior only (the model states only the error class). The recording shows: when a DECISION task token is passed to RespondActivityTaskCompleted, AWS refuses with HTTP 400, code `UnknownResourceFault`, message `Unknown activity, scheduledEventId = 2` — and 2 is not a constant: it is the `scheduledEventId` carried by the token, which for that call is the `DecisionTaskScheduled` eventId of the run (the check derives the expected value from the run's own history, as `decisionTaskStartedEventAttributes.scheduledEventId` of the `DecisionTaskStarted` event the decision task names). So the message's FIXED text is `Unknown activity, scheduledEventId = ` and its single VARIABLE part is the scheduledEventId the supplied token addresses, taken from the emulator's stored history for that token — whatever KIND of task token it is. AWS reads that id out of the token itself, so a token of the wrong kind still names the id of the task it really is a handle on; it never degrades to 0 just because no activity task matches.

Emulator: the refusal class and status are right (HTTP 400, `UnknownResourceFault`, no `ActivityTaskCompleted` appended, and the real activity token still completes afterwards — all those assertions pass), but the message is `Unknown activity, scheduledEventId = 0`. The cause is in the shared close path: `ComAmazonawsSwfSimpleworkflowserviceState::close_activity_task` (services/service-com-amazonaws-swf-simpleworkflowservice/src/state.rs:3040) resolves the token only through `activity_task_by_token` (src/state.rs:3288), which matches a token against stored `ActivityTaskStarted` events ONLY. A decision-task token matches nothing there, so the `else` arm at src/state.rs:3049 returns `unknown_activity_message(0)` — the hardcoded `0` documented at src/state.rs:3256-3261 as "the EventId member's own modelled default". Every non-activity token therefore collapses to the same id-less message.

Why: a client that reads the refusal to find out WHICH task the service rejected gets `0`, an eventId no history event ever carries. A worker or test harness correlating late/mis-routed terminal responses back to a task (the common pattern of logging or asserting on the scheduledEventId in the fault message) cannot distinguish "wrong token kind" from any other unresolvable token, and an assertion on the exact AWS message text fails against the emulator even though the error code and status match.

Fix: the token-resolution step feeding the message, in `crate::state::close_activity_task` / `activity_task_by_token` (services/service-com-amazonaws-swf-simpleworkflowservice/src/state.rs) — resolve the scheduledEventId the supplied token addresses for ANY task-token kind this service mints (decision-task tokens included, via the decision-task token derivation and the run's `DecisionTaskScheduled`/`DecisionTaskStarted` events) and pass that id to `unknown_activity_message`, keeping the `0` fallback only for a token that resolves to no stored task at all. The refusal class, status and the no-write behavior are already correct and must not change. The same path is shared by RespondActivityTaskFailed and RespondActivityTaskCanceled, so fixing it there fixes all three.

Traces: swf.decision-token-used-for-activity-response-rejected.

## Evidence
Suite: 51/66 passed (77.3%); swf 51/65 with 14 failures (12 of them owned by open issues or auto-filed scaffold-stub issues).

Cause 1 — [mismatch] swf.decision-token-used-for-activity-response-rejected:
  check_swf_decision_token_used_for_activity_response_rejected.py:67
  AssertionError: expected message 'Unknown activity, scheduledEventId = 2', got 'Unknown activity, scheduledEventId = 0'

  The check's preceding assertions all PASS, which bounds the defect precisely:
    - `assert code == "UnknownResourceFault"` (line 58 of the earlier revision / line 60 here) passes — the error class and HTTP 400 are correct.
    - no `ActivityTaskCompleted` event is appended by the refused call — correct.
  The expected value is computed by the check from the emulator's OWN history, not canned:
    decision_started = the event whose eventId == decision['startedEventId']
    decision_scheduled_id = decision_started['decisionTaskStartedEventAttributes']['scheduledEventId']
    expected_message = f"Unknown activity, scheduledEventId = {decision_scheduled_id}"
  so `2` is this run's DecisionTaskScheduled eventId filling the template, not a literal requirement.

  Expected: HTTP 400, Error.Code `UnknownResourceFault`, Error.Message = fixed text "Unknown activity, scheduledEventId = " + the scheduledEventId the supplied token addresses (here the run's DecisionTaskScheduled eventId).
  Actual:   HTTP 400, Error.Code `UnknownResourceFault`, Error.Message "Unknown activity, scheduledEventId = 0".

  Code path: src/ops/respond_activity_task_completed.rs:202-218 calls `guard.close_activity_task(&input.task_token, ...)` and wraps the returned String as the fault message; src/state.rs:3046-3049 — `let Some(handle) = activity_task_by_token(self, token) else { return Err(unknown_activity_message(0)); }`; src/state.rs:3288 `activity_task_by_token` searches stored `ActivityTaskStarted` events only, so a decision-task token never resolves; src/state.rs:3262 `unknown_activity_message` formats whatever id it is handed.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and point a botocore `swf` client at it.
2. RegisterDomain; RegisterWorkflowType; RegisterActivityType. StartWorkflowExecution on a decision task list.
3. PollForDecisionTask on that list and keep the returned `taskToken` (the DECISION token) and `startedEventId`.
4. RespondDecisionTaskCompleted with a single `ScheduleActivityTask` decision onto a separate activity task list; PollForActivityTask to get the real activity token (so the activity task is open and the run is healthy).
5. Call RespondActivityTaskCompleted(taskToken=<the DECISION token from step 3>, result="wrong-token").
6. Observe HTTP 400 / `UnknownResourceFault` with Error.Message "Unknown activity, scheduledEventId = 0". Expected: the same status and code, with the scheduledEventId the token addresses — i.e. GetWorkflowExecutionHistory, find the event whose eventId == the `startedEventId` from step 3, read `decisionTaskStartedEventAttributes.scheduledEventId`, and that value must appear in the message.
7. Confirm the refusal still has no side effect (no `ActivityTaskCompleted` in history) and that RespondActivityTaskCompleted with the real activity token from step 4 still returns 200 and records `ActivityTaskCompleted` — both already hold.
8. Replay: swf.decision-token-used-for-activity-response-rejected.
