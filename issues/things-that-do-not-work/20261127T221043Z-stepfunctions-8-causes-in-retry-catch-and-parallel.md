Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: stepfunctions.map-iteration-sdk-task-access-denied-raises-service-exception-not-states-permissions stepfunctions.parallel-branch-access-denial-caught-as-service-exception-not-states-permissions stepfunctions.task-access-denial-caught-as-service-exception-not-states-permissions stepfunctions.map-retry-max-delay-seconds-caps-fail-state-reruns stepfunctions.parallel-retry-backoff-rate-spaces-fail-state-reruns stepfunctions.retry-exhausted-into-catch-with-null-result-path-keeps-raw-state-input stepfunctions.task-parameters-over-payload-quota-not-retried-by-retrier-naming-it-rejected stepfunctions.task-parameters-string-to-json-failure-not-retried-as-intrinsic-failure-rejected stepfunctions.parallel-output-over-quota-fails-execution-past-its-named-catch stepfunctions.failing-parallel-branch-records-map-state-aborted-for-its-sibling-map stepfunctions.parallel-branch-failure-aborts-a-nested-parallel-in-flight stepfunctions.states-runtime-from-input-path-in-parallel-branch-escapes-named-catch-rejected stepfunctions.timeout-seconds-path-above-its-maximum-is-rejected stepfunctions.retrier-with-toleratedfailurepercentage-rejected-plain-retrier-counts-attempts
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: stepfunctions.map-iteration-sdk-task-access-denied-raises-service-exception-not-states-permissions|run#25|AssertionError|assert denied_out == {"route": "all", "error": _SDK_ERROR}, (
Issue-Key: stepfunctions.parallel-branch-access-denial-caught-as-service-exception-not-states-permissions|run#27|AssertionError|assert denied["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.task-access-denial-caught-as-service-exception-not-states-permissions|run#27|AssertionError|assert denied["status"] == "SUCCEEDED", (
Issue-Key: stepfunctions.map-retry-max-delay-seconds-caps-fail-state-reruns|run#21|AssertionError|assert kinds.count("MapStateFailed") == 1, (
Issue-Key: stepfunctions.parallel-retry-backoff-rate-spaces-fail-state-reruns|run#21|AssertionError|assert kinds.count("ParallelStateFailed") == 1, (
Issue-Key: stepfunctions.retry-exhausted-into-catch-with-null-result-path-keeps-raw-state-input|run#17|AssertionError|assert len(failed) == 1 and failed[0]["id"] > refused[-1]["id"], (
Issue-Key: stepfunctions.task-parameters-over-payload-quota-not-retried-by-retrier-naming-it-rejected|run#21|AssertionError|assert served == want, f"control task inputs {served!r}, expected {want!r}"
Issue-Key: stepfunctions.task-parameters-string-to-json-failure-not-retried-as-intrinsic-failure-rejected|run#18|AssertionError|assert got == [{"doc": {"n": 1}}, {"doc": {"n": 2, "tag": "b"}}], (
Issue-Key: stepfunctions.parallel-output-over-quota-fails-execution-past-its-named-catch|run#7|AssertionError|assert (failed["status"], failed.get("error")) == ("FAILED", _LIMIT_ERROR), (
Issue-Key: stepfunctions.failing-parallel-branch-records-map-state-aborted-for-its-sibling-map|run#13|AssertionError|assert (types.count("MapIterationAborted"), types.count("WaitStateAborted")) == (2, 2), (
Issue-Key: stepfunctions.parallel-branch-failure-aborts-a-nested-parallel-in-flight|_claim#9|Timeout|task = worker.get_activity_task(activityArn=activity_arn,
Issue-Key: stepfunctions.states-runtime-from-input-path-in-parallel-branch-escapes-named-catch-rejected|run#19|AssertionError|assert "Pick" in bad_path, f"the branch must have entered 'Pick', entered {bad_path!r}"
Issue-Key: stepfunctions.timeout-seconds-path-above-its-maximum-is-rejected|_finish#21|AssertionError|assert time.monotonic() < deadline, f"execution still RUNNING: {described!r}"
Issue-Key: stepfunctions.retrier-with-toleratedfailurepercentage-rejected-plain-retrier-counts-attempts|run#19|KeyError|e["activityFailedEventDetails"]["cause"] for e in history if e["type"] == "ActivityFailed"
Issue-Commit: 3c63a583c399d299d4ea298f38dfae7b708152ae

# stepfunctions: 8 causes in retry/catch and Parallel semantics — SDK denials raise the wrong task error, retried Map/Parallel states fail on every attempt, Parameters are not re-evaluated on retry, and the output quota and sibling-abort rules are not applied

> _Note: this issue was identified against the code base at commit `3c63a583c399d299d4ea298f38dfae7b708152ae`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Cause 1 of 8: an access denial from an AWS SDK integration is raised under the service's own error-code name instead of the generic service-exception name.
AWS: when an aws-sdk Task is denied, the task error is '<Service>.<ServiceExceptionName>' — for Step Functions' own SDK integration 'Sfn.SfnException' — so a Catcher naming States.Permissions does NOT take it and a States.ALL (or 'Sfn.SfnException') catcher does; the cause carries the IAM denial sentence.
Emulator: the error is 'Sfn.AccessDeniedException', so the States.ALL route sees the wrong name and, in two of the three cases, nothing catches it at all and the execution ends FAILED with 'Sfn.AccessDeniedException'.
Why: a workflow's error-handling routes on the task error name; with the wrong name a denial escapes the catcher that is meant to handle it and fails the execution.
Fix: the error naming for aws-sdk integration failures in the Task executor (map a service refusal to '<Service>.<ServiceExceptionName>').
Traces: stepfunctions.map-iteration-sdk-task-access-denied-raises-service-exception-not-states-permissions, stepfunctions.parallel-branch-access-denial-caught-as-service-exception-not-states-permissions, stepfunctions.task-access-denial-caught-as-service-exception-not-states-permissions.

Cause 2 of 8: a retried Map or Parallel state records a state failure on every attempt instead of only the final one.
AWS: when a Retrier reruns a Map or Parallel state, the history records the state's failure once — after the last attempt — so a succeeding retry leaves exactly one MapStateFailed/ParallelStateFailed (and a retry exhausted into a Catch records one failure, not one per attempt).
Emulator: each attempt appends its own failure: ['...','MapStateStarted','MapIterationStarted','FailStateEntered','MapIterationFailed','MapStateFailed', 'MapStateStarted', ... 'MapStateFailed', ...] (four times), the same for ParallelStateFailed, and the retry-into-catch case shows 3 failures where 1 is expected.
Why: the history reports failures that never happened on AWS, so attempt counting, failure metrics and any history-driven assertion are wrong.
Fix: the retry wrapper around compound states (Map/Parallel) in the execution engine — only the terminal attempt emits the state-failed event.
Traces: stepfunctions.map-retry-max-delay-seconds-caps-fail-state-reruns, stepfunctions.parallel-retry-backoff-rate-spaces-fail-state-reruns, stepfunctions.retry-exhausted-into-catch-with-null-result-path-keeps-raw-state-input.

Cause 3 of 8: a Task's Parameters are not re-evaluated on a retry attempt.
AWS: each retry attempt re-evaluates the state's Parameters against the state input, so an attempt-dependent payload differs between attempts (attempt 0 sees 'tiny', attempt 1 sees 'small'), and an intrinsic inside Parameters (States.StringToJson) is re-run so each attempt receives the item it picked.
Emulator: both attempts receive the attempt-0 payload — [{'c1': 'tiny', ..., 'attempt': 0}, {'c1': 'tiny', ..., 'attempt': 0}] and [{'doc': {'n': 1}}, {'doc': {'n': 1}}] — the first evaluation is cached and replayed.
Why: retries that depend on re-evaluated input (back-off payload shrinking, attempt counters, re-parsing) send the wrong request on every attempt after the first.
Fix: Parameters/Arguments evaluation per attempt in the Task executor's retry loop.
Traces: stepfunctions.task-parameters-over-payload-quota-not-retried-by-retrier-naming-it-rejected, stepfunctions.task-parameters-string-to-json-failure-not-retried-as-intrinsic-failure-rejected.

Cause 4 of 8: a Parallel branch whose output exceeds the 256 KiB state payload quota does not fail the execution with States.DataLimitExceeded.
AWS: an output over the quota fails the execution with error 'States.DataLimitExceeded', and a Catcher that does not name it does not take it.
Emulator: the execution ends 'SUCCEEDED' with error None — the oversized output is carried through.
Why: the documented payload quota is not enforced, so a workflow that AWS fails runs here and the quota error a client handles never appears.
Fix: the state-output size check in the Parallel/branch result handling of the execution engine.
Traces: stepfunctions.parallel-output-over-quota-fails-execution-past-its-named-catch.

Cause 5 of 8: when one branch fails, in-flight sibling branches are not aborted with their nested states.
AWS: a failing Parallel branch aborts its siblings: a sibling Map in flight records MapIterationAborted for each waiting iteration alongside WaitStateAborted/MapStateAborted, and a nested Parallel in flight is aborted too.
Emulator: only one WaitStateAborted/MapStateAborted pair is recorded for two waiting iterations (['...','MapIterationStarted','MapIterationStarted','WaitStateEntered','WaitStateAborted','MapStateAborted',...]), and the nested-Parallel case is killed at the 60 s harness cap — the abort does not propagate.
Why: the history does not show the iterations that were cancelled, and work the service must stop keeps running (or hangs), so cancellation cannot be relied on.
Fix: the abort propagation for in-flight child states in the Parallel/Map executors.
Traces: stepfunctions.failing-parallel-branch-records-map-state-aborted-for-its-sibling-map, stepfunctions.parallel-branch-failure-aborts-a-nested-parallel-in-flight.

Cause 6 of 8: a States.Runtime raised by InputPath inside a Parallel branch takes the wrong transition.
AWS: the InputPath failure escapes a Catcher that does not name States.Runtime, and the branch proceeds into its 'Pick' state as recorded in the history.
Emulator: the branch only ever enters ['Fanout', 'Gate'] — 'Pick' is never entered.
Why: the branch follows a different path than on AWS, so the workflow's result and history differ.
Fix: error classification and Catch matching for InputPath evaluation failures inside Parallel branches in the execution engine.
Traces: stepfunctions.states-runtime-from-input-path-in-parallel-branch-escapes-named-catch-rejected.

Cause 7 of 8: a TimeoutSecondsPath resolving above the maximum is not refused, and the execution hangs.
AWS: a TimeoutSecondsPath whose resolved value exceeds the allowed maximum fails the execution (it does not stay RUNNING).
Emulator: the execution is still RUNNING when the check gives up — status 'RUNNING' with input '{"budget": 100000000}'.
Why: an out-of-range timeout leaves an execution running indefinitely instead of failing, so the run never reaches a terminal status.
Fix: TimeoutSeconds/TimeoutSecondsPath range validation in the Task executor (and at definition validation where the bound is static).
Traces: stepfunctions.timeout-seconds-path-above-its-maximum-is-rejected.

Cause 8 of 8: a Retrier carrying ToleratedFailurePercentage is not refused, and the resulting failure has no cause.
AWS: ToleratedFailurePercentage is not a Retrier field; a definition using it is refused, and the plain retrier's failure details carry a `cause`.
Emulator: the check fails with KeyError: 'cause' — the failure details hold no cause member at all.
Why: an invalid Retrier field is accepted, and the failure it produces cannot be diagnosed because the cause is missing.
Fix: Retrier field validation in the definition validator, and the cause member of the failure details the retry path records.
Traces: stepfunctions.retrier-with-toleratedfailurepercentage-rejected-plain-retrier-counts-attempts.

## Evidence
Cause 1: "the denied SDK call must be caught by States.ALL as 'Sfn.SfnException', passing over the States.Permissions catcher; got {'route': 'all', 'error': 'Sfn.AccessDeniedException'}"; "the denied branch must be caught, but the execution ended 'FAILED': 'Sfn.AccessDeniedException' 'User: ... is not authorized to perform: states:DescribeActivity ...'"; same for the Task case.
Cause 2: "only the exhausted last run may fail the Map state; history [... 'MapStateFailed', 'MapStateStarted', ... 'MapStateFailed' ...]"; "only the exhausted last run may fail the Parallel state; history [... 'ParallelStateFailed' x3 ...]"; "the state must fail once, after its last attempt; history shows 3 failures".
Cause 3: "control task inputs [{...'attempt': 0}, {...'attempt': 0}], expected [{...'attempt': 0}, {'c1': 'small', ..., 'attempt': 1}]"; "task inputs [{'doc': {'n': 1}}, {'doc': {'n': 1}}], expected each attempt to receive the parsed item it picked".
Cause 4: "an output over the quota from a branch state must fail the execution with States.DataLimitExceeded; got 'SUCCEEDED' error=None".
Cause 5: "both waiting iterations should be aborted along with their Wait: ['ExecutionStarted','ParallelStateEntered','ParallelStateStarted','MapStateEntered','MapStateStarted','MapIterationStarted','MapIterationStarted','WaitStateEntered','WaitStateAborted','MapStateAborted',...]"; and "Error: Check timed out after 60.0s" for the nested-Parallel case.
Cause 6: "the branch must have entered 'Pick', entered ['Fanout', 'Gate']".
Cause 7: "execution still RUNNING: {... 'status': 'RUNNING', 'input': '{\"budget\": 100000000}' ...}".
Cause 8: KeyError: 'cause'.

## Steps to Reproduce
1. Run a Task / Parallel branch / Map iteration whose aws-sdk call is denied by IAM and read the task error name and which catcher takes it (cause 1).
2. Attach a Retrier to a Map and to a Parallel state that keeps failing, and to a Task that exhausts into a Catch, then read GetExecutionHistory for one-vs-per-attempt state failures (cause 2).
3. Retry a Task whose Parameters depend on the attempt (and on States.StringToJson) and compare the payload each attempt receives (cause 3).
4. Return more than 256 KiB from a Parallel branch state (cause 4).
5. Fail one Parallel branch while a sibling Map / nested Parallel is in flight (cause 5); raise a States.Runtime from InputPath inside a Parallel branch with a catcher that does not name it (cause 6).
6. Start an execution whose TimeoutSecondsPath resolves above the maximum (cause 7), and create a definition whose Retrier carries ToleratedFailurePercentage (cause 8).
7. Or replay the check ids in trace_ids.
