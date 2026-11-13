Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: stepfunctions.activity-heartbeatsecondspath-timeout-fails-execution stepfunctions.activity-stopped-execution-refuses-its-outstanding-token stepfunctions.activity-token-consumed-by-failure-refuses-a-later-success stepfunctions.error-task-does-not-exist-heartbeat stepfunctions.redrive-execution-resumes-from-aborted-status stepfunctions.send-task-failure-after-timeout-token-invalid stepfunctions.send-task-failure-invalid-token stepfunctions.send-task-heartbeat-after-timeout-fails stepfunctions.send-task-success-after-completion-rejected stepfunctions.send-task-success-invalid-token-after-timeout stepfunctions.send-task-success-token-reuse-fails stepfunctions.task-timed-out-error-on-heartbeat-after-window
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: stepfunctions.activity-heartbeatsecondspath-timeout-fails-execution|run#17|AssertionError|assert message == f"Task Timed Out: '{activity_arn}'", (
Issue-Key: stepfunctions.activity-stopped-execution-refuses-its-outstanding-token|run#23|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: stepfunctions.activity-token-consumed-by-failure-refuses-a-later-success|run#23|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: stepfunctions.error-task-does-not-exist-heartbeat|run#20|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: sfn|SendTaskSuccess|TaskTimedOut
Issue-Key: stepfunctions.send-task-failure-after-timeout-token-invalid|run#20|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: stepfunctions.send-task-failure-invalid-token|run#13|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: stepfunctions.send-task-heartbeat-after-timeout-fails|run#14|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: stepfunctions.send-task-success-after-completion-rejected|run#14|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: stepfunctions.send-task-success-invalid-token-after-timeout|run#17|AssertionError|assert error.get("Message") == expected_message, (
Issue-Key: stepfunctions.send-task-success-token-reuse-fails|run#13|AssertionError|assert error.get("Message") == expected_message, (
Issue-Key: stepfunctions.task-timed-out-error-on-heartbeat-after-window|run#13|AssertionError|assert message == f"Task Timed Out: '{activity_arn}'", f"unexpected TaskTimedOut message: {message!r}"
Issue-Commit: 23d7c3ca3e68cb70ca93342274fc19d1394baa26

# stepfunctions: every TaskTimedOut refusal (SendTaskSuccess / SendTaskFailure / SendTaskHeartbeat) substitutes the literal 'Provided task does not exist anymore' for the activity ARN AWS quotes

> _Note: this issue was identified against the code base at commit `23d7c3ca3e68cb70ca93342274fc19d1394baa26`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendTaskSuccess

## Description
One root cause, reached by 12 distinct behavioral checks across all three task-token callback APIs.

Cause 1 of 1: the TaskTimedOut refusal the emulator returns for a dead/expired/consumed activity task token carries a fixed placeholder sentence in the slot where AWS quotes the activity ARN whose task the token belonged to. Failing check ids: stepfunctions.activity-heartbeatsecondspath-timeout-fails-execution, stepfunctions.activity-stopped-execution-refuses-its-outstanding-token, stepfunctions.activity-token-consumed-by-failure-refuses-a-later-success, stepfunctions.error-task-does-not-exist-heartbeat, stepfunctions.redrive-execution-resumes-from-aborted-status, stepfunctions.send-task-failure-after-timeout-token-invalid, stepfunctions.send-task-failure-invalid-token, stepfunctions.send-task-heartbeat-after-timeout-fails, stepfunctions.send-task-success-after-completion-rejected, stepfunctions.send-task-success-invalid-token-after-timeout, stepfunctions.send-task-success-token-reuse-fails, stepfunctions.task-timed-out-error-on-heartbeat-after-window.

AWS: Smithy (sfn, sdkId Step Functions) models SendTaskSuccess (input taskToken @required @length 1..2048 @sensitive, output @required @length 0..262144 @sensitive), SendTaskFailure (taskToken, error, cause) and SendTaskHeartbeat (taskToken) each with the client error TaskTimedOut, httpError 400 (defaulted), alongside TaskDoesNotExist and InvalidToken. The error's MESSAGE text is not modeled, so it is recorded behavior only. Real AWS answers all three operations, for a token whose task is no longer live — because the task's HeartbeatSeconds window elapsed, because the Task state's TimeoutSeconds elapsed, because the token was already consumed by a prior SendTaskSuccess or SendTaskFailure, or because the execution holding it was stopped — with 400 TaskTimedOut and the message `Task Timed Out: '<X>'`. The fixed text is `Task Timed Out: ` plus a single-quoted VARIABLE part resolved from the emulator's own state: the ARN of the activity that handed out that task token via GetActivityTask. (The particular ARN in any one run is a per-run value and is not part of the requirement; the requirement is that the quoted slot holds the token's activity ARN.) The refusal must also leave the already-terminal execution's record untouched — status, error, cause and output unchanged — which the checks confirm separately.

Emulator: the HTTP status (400) and the error CODE (TaskTimedOut) are correct, but the message is always `Task Timed Out: 'Provided task does not exist anymore'` — a constant English sentence occupies the quoted variable slot instead of the activity ARN. All 12 checks fail on exactly this one string, for all three callback operations and for every way the token can die (heartbeat window elapsed, TimeoutSeconds elapsed, token consumed by success, token consumed by failure, execution stopped, execution aborted and redriven), which shows the text is hard-coded in one shared error construction rather than per-path.

Why: a client relying on AWS's behavior parses the quoted segment to learn WHICH activity's task died — it is the only identifying information in the refusal, since the task token itself is opaque and @sensitive. A worker fleet polling several activities cannot attribute the refusal; per-activity retry, requeue and alerting logic keyed on the ARN never matches; and the text actively misinforms operators, saying the task 'does not exist' when the modelled, distinct condition for that is TaskDoesNotExist while the real condition here is that the task timed out or was already answered.

Fix: the single TaskTimedOut error construction shared by the stepfunctions SendTaskSuccess, SendTaskFailure and SendTaskHeartbeat handlers (the activity/task-token callback path and its state lookup). It must resolve the presented token to the task it was issued for, read that task's activity ARN out of execution state, and interpolate that ARN into the quoted slot after the fixed `Task Timed Out: ` prefix, instead of emitting a hard-coded sentence. No other part of these refusals needs to change.

Related already-open issue: 20261113T184407Z-cloudwatch-stepfunctions-sendtasksuccess-tasktimedout-message-substitutes-the-literal-provided-task.md describes the same message defect but is grounded in, and carries the replay key of, a single cloudwatch__stepfunctions check only; it does not cover any of the 12 stepfunctions checks listed here, which the suite reports as unowned. Fixing the shared construction should close both.

Traces: the 12 failing check ids listed above; representative evidence below.

## Evidence
Checks (all verdict 'mismatch', service heading `### stepfunctions`), all failing on the same assertion:

- stepfunctions.send-task-success-after-completion-rejected (line 42 of the check): `AssertionError: expected message "Task Timed Out: 'arn:aws:states:us-west-2:000000000000:activity:sim2real-sfn-activity-936104f48649'", got "Task Timed Out: 'Provided task does not exist anymore'"`. The check first asserted `code == "TaskTimedOut"` and that passed.
- stepfunctions.task-timed-out-error-on-heartbeat-after-window: `AssertionError: unexpected TaskTimedOut message: "Task Timed Out: 'Provided task does not exist anymore'"` after an in-window SendTaskHeartbeat succeeded (positive control) and the execution went FAILED on the elapsed window.
- stepfunctions.error-task-does-not-exist-heartbeat: SendTaskHeartbeat on a token already spent by SendTaskSuccess -> code TaskTimedOut (correct), message `Task Timed Out: 'Provided task does not exist anymore'` instead of `Task Timed Out: '<activityArn>'`.
- stepfunctions.activity-stopped-execution-refuses-its-outstanding-token, stepfunctions.activity-token-consumed-by-failure-refuses-a-later-success, stepfunctions.send-task-failure-invalid-token, stepfunctions.send-task-failure-after-timeout-token-invalid, stepfunctions.send-task-heartbeat-after-timeout-fails, stepfunctions.send-task-success-invalid-token-after-timeout, stepfunctions.send-task-success-token-reuse-fails, stepfunctions.activity-heartbeatsecondspath-timeout-fails-execution, stepfunctions.redrive-execution-resumes-from-aborted-status: identical diff, each expecting `Task Timed Out: '<that run's activity ARN>'` and receiving `Task Timed Out: 'Provided task does not exist anymore'`.

Expected (real AWS): HTTP 400, Error.Code `TaskTimedOut`, Error.Message == `Task Timed Out: '` + <ARN of the activity that issued the token, from state> + `'`. Fixed text: `Task Timed Out: ` and the surrounding single quotes. Variable text: the activity ARN (state-derived).
Actual (emulator): HTTP 400, Error.Code `TaskTimedOut`, Error.Message == `Task Timed Out: 'Provided task does not exist anymore'` — variable slot filled with a constant literal.

In every one of the 12 checks the surrounding behavior passed: the positive controls (live token accepted, in-window heartbeat accepted, fresh execution succeeding with its own output) passed, the token was really issued by GetActivityTask, the task really died the way the check arranged, and the terminal execution record was left unchanged. Only the refusal's message text diverges.

## Steps to Reproduce
1. CreateActivity(name=<activity>) -> <activityArn>.
2. CreateStateMachine (STANDARD) on a states.amazonaws.com-trusted role, with a single Task state: {"Type":"Task","Resource":"<activityArn>","TimeoutSeconds":300,"HeartbeatSeconds":5,"End":true}.
3. StartExecution(stateMachineArn=<sm>, input="{}"), then GetActivityTask(activityArn=<activityArn>, workerName="w1") to obtain <token>.
4. Positive control: SendTaskHeartbeat(taskToken=<token>) while the window is fresh — accepted.
5. Let the 5s HeartbeatSeconds window elapse untouched; DescribeExecution reports status FAILED.
6. Call any of: SendTaskHeartbeat(taskToken=<token>), SendTaskFailure(taskToken=<token>, error="Worker.LateReport", cause="found out too late"), SendTaskSuccess(taskToken=<token>, output="{}").
   -> AWS: 400 TaskTimedOut, message "Task Timed Out: '<activityArn>'".
   -> Emulator: 400 TaskTimedOut, message "Task Timed Out: 'Provided task does not exist anymore'".
7. The same divergence appears with the token killed any other way: answer it once with SendTaskSuccess (or SendTaskFailure) and call again; or StopExecution while the token is outstanding and then call SendTaskSuccess — in each case AWS quotes <activityArn> and the emulator emits the literal sentence.
