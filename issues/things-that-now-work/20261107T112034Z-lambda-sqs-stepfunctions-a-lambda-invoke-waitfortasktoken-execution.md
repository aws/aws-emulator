Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sqs__stepfunctions.sfn-task-token-duplicate-sendtasksuccess-rejected-negative
Issue-Key: lambda__sqs__stepfunctions.sfn-task-token-duplicate-sendtasksuccess-rejected-negative|_run_until_succeeded#7|Timeout|time.sleep(retry_backoff_s)
Issue-Commit: 46dd75ad5ad4e002f82bb21fdacb370faf08a983

# lambda__sqs__stepfunctions: a lambda:invoke.waitForTaskToken execution never reaches SUCCEEDED, so a replayed SendTaskSuccess on a closed token is never rejected with TaskTimedOut

> _Note: this issue was identified against the code base at commit `46dd75ad5ad4e002f82bb21fdacb370faf08a983`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendTaskSuccess

## Description
One net-new root cause in this scenario service.

Cause 1 of 1: the optimized `arn:aws:states:::lambda:invoke.waitForTaskToken` callback round-trip never completes, so the whole task-token close/replay contract is unobservable.

AWS: recorded behavior only for the callback resumption (Step Functions' Smithy model declares SendTaskSuccess with input taskToken (TaskToken, required, @length 1-2048, sensitive) and output (SensitiveData, required, @length 0-262144, sensitive) and an empty output shape, and models its errors as InvalidOutput, InvalidToken, TaskDoesNotExist, TaskTimedOut and the Kms* family — but it does not model Amazon States Language resumption semantics). The recording shows that a Task whose Resource is `arn:aws:states:::lambda:invoke.waitForTaskToken` invokes the function with the Parameters payload (here carrying `$$.Task.Token`), suspends the execution in RUNNING, and that the function's own `boto3.client('stepfunctions').send_task_success(taskToken=..., output=...)` completes the task so the execution becomes SUCCEEDED with DescribeExecution.output equal to the output the callback sent (the value is whatever the caller passed, never a literal from one run). The recording further shows that a SECOND SendTaskSuccess carrying that now-closed token is refused with HTTP 400, Error.Code `TaskTimedOut` and Error.Message exactly `Task Timed Out: 'Provided task does not exist anymore'` — that message is entirely fixed text for this error class, with no variable part — and that the refusal is inert: DescribeExecution for the already-succeeded execution still reports status SUCCEEDED with its original output unchanged.

Emulator: the positive control never settles. The check starts fresh executions of that state machine on a bounded deadline and none ever reaches SUCCEEDED, so the check exhausts its budget (`Check timed out after 60.0s`) inside its start-and-poll retry loop and never gets as far as issuing the replayed SendTaskSuccess. Consequently nothing is known about the emulator's TaskTimedOut refusal or about the refusal being inert — both remain unverified because the first, successful callback never happens. Expected: the first execution SUCCEEDED carrying the callback output; the replay refused 400 / TaskTimedOut / "Task Timed Out: 'Provided task does not exist anymore'"; DescribeExecution afterwards unchanged.

Why: a client using the documented Lambda callback pattern (`lambda:invoke.waitForTaskToken`) gets an execution that hangs in RUNNING forever instead of finishing, and a client that defends against duplicate/retried callbacks cannot rely on the TaskTimedOut refusal at all — a replayed SendTaskSuccess has undefined behavior here and could silently mutate a completed execution's recorded outcome.

Fix: the Step Functions interpreter's optimized-integration + task-token callback path — the `states:::lambda:invoke.waitForTaskToken` task integration, the task-token registry that must close a token once SendTaskSuccess lands and drive the execution to SUCCEEDED with the sent output, and the SendTaskSuccess handler's validation that must reject a closed/unknown token with the modeled TaskTimedOut error (status 400, code TaskTimedOut, message `Task Timed Out: 'Provided task does not exist anymore'`) while leaving the execution's recorded state untouched. Related but NOT covering this check: 20261107T044335Z-lambda-sqs-stepfunctions-waitfortasktoken-callbacks-never-resume-the.md owns only the `sqs:sendMessage.waitForTaskToken` resume path, the uncodeable denied SendTaskSuccess and the heartbeat-timeout `cause`; 20261107T081525Z-lambda-sqs-stepfunctions-3-net-new-causes-a.md owns the Map/MaxConcurrency failure, the missing in-function `stepfunctions` SDK client and the SQS ESM scale-out. Neither carries this check's replay key, and this check was not marked owned by either in this run; if the in-function SDK gap is the proximate reason the callback never fires, the `lambda:invoke.waitForTaskToken` closure and the TaskTimedOut replay refusal still have to be verified under this check before it is closed.

Traces: lambda__sqs__stepfunctions.sfn-task-token-duplicate-sendtasksuccess-rejected-negative.

## Evidence
Cause 1 — expected: the first execution of a STANDARD state machine whose single Task is Resource `arn:aws:states:::lambda:invoke.waitForTaskToken` (Parameters {FunctionName: <fn arn>, Payload: {"token.$": "$$.Task.Token"}}) reaches DescribeExecution status SUCCEEDED carrying the output the function passed to SendTaskSuccess; then a replayed `send_task_success(taskToken=<that token>, output=...)` is refused with HTTP 400, Error.Code 'TaskTimedOut', Error.Message "Task Timed Out: 'Provided task does not exist anymore'"; then DescribeExecution still reports SUCCEEDED with the identical output.
Actual: `Check timed out after 60.0s` — no execution of that state machine ever reached SUCCEEDED within the check's 90s start-and-retry deadline, so the check never issued the replayed SendTaskSuccess and the TaskTimedOut refusal and its inertness were never exercised. Smithy (sfn): SendTaskSuccess errors = InvalidOutput, InvalidToken, KmsAccessDeniedException, KmsInvalidStateException, KmsThrottlingException, TaskDoesNotExist, TaskTimedOut (all client, HTTP 400); taskToken @length 1-2048; output @length 0-262144.

## Steps to Reproduce
1. Create an IAM role trusted by lambda.amazonaws.com granting states:SendTaskSuccess, and an IAM role trusted by states.amazonaws.com granting lambda:InvokeFunction.
2. Create a python3.12 Lambda whose handler does: token = event['token']; boto3.client('stepfunctions').send_task_success(taskToken=token, output=json.dumps({'token': token})); return {'sent': True}.
3. Create a STANDARD state machine: {"StartAt":"WaitForToken","States":{"WaitForToken":{"Type":"Task","Resource":"arn:aws:states:::lambda:invoke.waitForTaskToken","Parameters":{"FunctionName":"<fn arn>","Payload":{"token.$":"$$.Task.Token"}},"End":true}}}.
4. StartExecution with input {} and poll DescribeExecution. Observed: the execution never becomes SUCCEEDED (the check retries fresh executions until its 60s budget is gone); AWS settles it SUCCEEDED with output {"token": "<the task token>"}.
5. Once step 4 succeeds, call SendTaskSuccess again with the same token: it must be refused with HTTP 400, code TaskTimedOut and message "Task Timed Out: 'Provided task does not exist anymore'", and a subsequent DescribeExecution must still report SUCCEEDED with the unchanged output.
