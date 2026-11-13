Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__stepfunctions.metrics-states-stream-late-activity-result-rejected-after-task-timeout-and-only-timed-out-moves
Issue-Key: cloudwatch__stepfunctions.metrics-states-stream-late-activity-result-rejected-after-task-timeout-and-only-timed-out-moves|run#21|AssertionError|assert message == f"Task Timed Out: '{slow_act}'", f"TaskTimedOut message {message!r}"
Issue-Commit: 9ca58d7cf30a6ec744c8cbd01b69502e157676e2

# SendTaskSuccess TaskTimedOut message substitutes the literal 'Provided task does not exist anymore' for the activity ARN AWS quotes

> _Note: this issue was identified against the code base at commit `9ca58d7cf30a6ec744c8cbd01b69502e157676e2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendTaskSuccess

## Description
Cause 1 of 1: the TaskTimedOut refusal for a late activity answer carries a fixed placeholder sentence where AWS quotes the activity ARN whose task expired. Failing check id: cloudwatch__stepfunctions.metrics-states-stream-late-activity-result-rejected-after-task-timeout-and-only-timed-out-moves.

AWS: Smithy (sfn) models SendTaskSuccess with the error TaskTimedOut (errorKind client, httpError 400) carrying a `message` member; the message TEXT is not modeled, so it is recorded behavior only. Real AWS refuses SendTaskSuccess carrying a task token whose Task state already exceeded its TimeoutSeconds with 400 TaskTimedOut and the message `Task Timed Out: '<X>'` — fixed text `Task Timed Out: ` followed by a single-quoted VARIABLE part taken from the emulator's own state: the ARN of the activity that handed out that task token. The execution itself is already FAILED with error States.Timeout and the refused late answer must leave it untouched (status FAILED, no output).

Emulator: the error CODE is right (TaskTimedOut) but the message is `Task Timed Out: 'Provided task does not exist anymore'` — the quoted segment is a constant English sentence instead of the activity ARN the token belongs to. The harness reports `AssertionError: TaskTimedOut message "Task Timed Out: 'Provided task does not exist anymore'"` where the expectation is `Task Timed Out: '<activityArn>'`. The particular ARN in any one run is a per-run value; the requirement is that the quoted part is the task's activity ARN resolved from the token, not a literal.

Why: a client relying on AWS's behavior parses the quoted segment to learn WHICH activity's task expired — that is the only identifying information in the refusal, since the token is opaque and sensitive. With a constant sentence there, a worker fleet polling several activities cannot attribute the timeout, per-activity retry/alerting logic keyed on the ARN never matches, and operators reading the message are told the task "does not exist" when the real condition is that it timed out.

Fix: the stepfunctions SendTaskSuccess handler's TaskTimedOut error construction (and the same construction on SendTaskFailure / SendTaskHeartbeat) — it must resolve the token to its task's activity ARN from execution state and interpolate that ARN into the quoted slot, instead of emitting a hard-coded sentence. The open issue 20261110T140310Z-cloudwatch-stepfunctions-an-activity-task-never-hands-out.md described the earlier state in which an activity Task never handed out a token at all, so the refusal was unreachable; a token is now handed out and the task does time out and is refused, so this message defect is net-new and not covered there.

Traces: cloudwatch__stepfunctions.metrics-states-stream-late-activity-result-rejected-after-task-timeout-and-only-timed-out-moves.

## Evidence
Check cloudwatch__stepfunctions.metrics-states-stream-late-activity-result-rejected-after-task-timeout-and-only-timed-out-moves, verdict mismatch:

AssertionError: TaskTimedOut message "Task Timed Out: 'Provided task does not exist anymore'"

Expected (real AWS): SendTaskSuccess(taskToken=<token of the timed-out activity task>, output=...) -> 400 TaskTimedOut, message == "Task Timed Out: '<arn of the activity that issued the token>'" (fixed text "Task Timed Out: " plus the single-quoted activity ARN from state).
Actual (emulator): 400 TaskTimedOut with message "Task Timed Out: 'Provided task does not exist anymore'" — the quoted variable slot is filled with a constant literal rather than the activity ARN.

The preceding surface of the same check passed, so the activity token was issued, the Task state did exceed TimeoutSeconds and the execution did reach FAILED with error States.Timeout; only the refusal's message text diverges.

## Steps to Reproduce
1. CreateActivity(name=<activity>) and CreateStateMachine whose single Task state has Resource=<activity arn> and TimeoutSeconds=5.
2. StartExecution, then GetActivityTask(activityArn=<activity arn>) to obtain the task token.
3. Wait for the Task to exceed TimeoutSeconds; DescribeExecution reports status FAILED and error "States.Timeout".
4. Call SendTaskSuccess(taskToken=<the token from step 2>, output='{"late": true}'): AWS answers 400 TaskTimedOut with message "Task Timed Out: '<activity arn>'"; the emulator answers 400 TaskTimedOut with message "Task Timed Out: 'Provided task does not exist anymore'".
