Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__iam__lambda__logs__sts.durable-execution-step-exhausts-retries-fails-execution dynamodb__iam__lambda__logs__sts.durable-execution-step-retry-recovers-and-replay-skips-completed-steps
Issue-Key: dynamodb__iam__lambda__logs__sts.durable-execution-step-exhausts-retries-fails-execution|_poll_until#6|TimeoutError|raise TimeoutError(f"the run budget ended first; not graded; last={last!r}")
Issue-Key: dynamodb__iam__lambda__logs__sts.durable-execution-step-retry-recovers-and-replay-skips-completed-steps|_poll_until#6|TimeoutError|raise TimeoutError(f"the run budget ended first; not graded; last={last!r}")
Issue-Commit: b5b438418db974c5623067148485e4df5aeb5fcd

# dynamodb__iam__lambda__logs__sts: a durable execution never advances past RUNNING — no step retry, no terminal status, no Error object

> _Note: this issue was identified against the code base at commit `b5b438418db974c5623067148485e4df5aeb5fcd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDurableExecution

## Description
Cause 1 of 1: a Lambda durable execution the emulator accepts never makes progress — GetDurableExecution keeps answering Status 'RUNNING' for the whole life of the poll, so neither the retry-exhaustion path nor the retry-recovery/replay path is ever reachable. Failing checks: dynamodb__iam__lambda__logs__sts.durable-execution-step-exhausts-retries-fails-execution, dynamodb__iam__lambda__logs__sts.durable-execution-step-retry-recovers-and-replay-skips-completed-steps.

AWS: Smithy (lambda) GetDurableExecution -> GET /2025-12-01/durable-executions/{DurableExecutionArn}, 200; output member Status targets ExecutionStatus, an enum whose values are exactly RUNNING | SUCCEEDED | FAILED | TIMED_OUT | STOPPED, and the output also carries EndTimestamp, Result and Error (shape ErrorObject with members ErrorMessage, ErrorType, ErrorData, StackTrace). Recorded behavior: a durable handler decorated with @durable_execution runs its steps; a @durable_step that raises on every attempt under a StepConfig retry strategy of max_attempts=2 checkpoints one StepFailed per attempt (StepFailedDetails.RetryDetails.CurrentAttempt = 1 then 2) and no StepSucceeded, the step after it is never scheduled, and the execution leaves RUNNING for the terminal status FAILED with Error.ErrorType 'CallableRuntimeError' and Error.ErrorMessage equal to the step's own exception message (fixed text comes from the handler, e.g. 'permanent failure, never recovers' in the recording; the variable part is whatever the step raised). The twin scenario, where the step succeeds on its second attempt, ends SUCCEEDED and the replay skips the already-checkpointed steps. Either way the execution reaches a terminal status within seconds, well inside the function's ExecutionTimeout of 300.

Emulator: the invoke is accepted (a DurableExecutionArn is returned) and GetDurableExecution answers HTTP 200 with Status 'RUNNING', ExecutionDataIncluded true, DurableConfig {RetentionPeriodInDays: 1, ExecutionTimeout: 300} and NO EndTimestamp, Result or Error — and it keeps answering exactly that on every 3 s poll until the replay budget runs out (last observation ~48 s after StartTimestamp for both checks). No step checkpoint, no retry, no terminal status ever appears, for the failing-step scenario or the recovering one.

Why: a client that starts a durable execution and polls for its outcome — the only way the API exposes one — waits forever. It can never observe FAILED/SUCCEEDED, can never read Error.ErrorType/ErrorMessage to decide whether the failure is retryable, and can never see the StepFailed/StepSucceeded history that the durable programming model's at-most-once step guarantee is asserted through.

Fix: the durable-execution engine in the Lambda crate — the step scheduler/retry driver that is supposed to advance an accepted execution through its steps, write StepFailed/StepSucceeded checkpoints, and settle the execution into a terminal ExecutionStatus with the Error object — plus the GetDurableExecution/history read path that serves them. Related open issues describe neighbouring causes but do not carry these checks' replay keys: 20261109T143515Z-dynamodb-iam-lambda-logs-sts-the-in-process-lambda-runtime-has-no-aws.md (the runtime lacked aws_durable_execution_sdk_python, so the handler could not even be imported — the execution now starts, which is past that symptom) and 20261027T203721Z-lambda-a-durable-execution-is-accepted-but-never-runs.md (same stuck-RUNNING shape for a different check set and service token).

Traces: the two failing check ids listed above.

## Evidence
Both checks poll GetDurableExecution every 3 s for a terminal status and never get one.

Expected (AWS): Status 'FAILED' with Error.ErrorType 'CallableRuntimeError' and Error.ErrorMessage = the step's own exception message, exactly two StepFailed checkpoints for the failing step with RetryDetails.CurrentAttempt 1 and 2, no StepSucceeded for it, the following step never scheduled, and the earlier step's single StepSucceeded intact (durable-execution-step-exhausts-retries-fails-execution); Status 'SUCCEEDED' with the replay skipping completed steps (durable-execution-step-retry-recovers-and-replay-skips-completed-steps).

Actual (emulator), last observation of both checks: HTTP 200 with {'DurableExecutionArn': 'arn:aws:lambda:<region>:<account>:function:<fn>:$LATEST/durable-execution/<name>/<id>', 'DurableExecutionName': '<name>', 'FunctionArn': '<fn arn>', 'InputPayload': '<the request payload>', 'StartTimestamp': <start>, 'Status': 'RUNNING', 'Version': '$LATEST', 'ExecutionDataIncluded': True, 'DurableConfig': {'RetentionPeriodInDays': 1, 'ExecutionTimeout': 300}} — no EndTimestamp, no Result, no Error. Status was still 'RUNNING' ~48 s after StartTimestamp in both runs, and the poll ended on the replay budget rather than on a terminal status.

## Steps to Reproduce
1. CreateTable (a counter table keyed on StepId) and create an execution role allowed dynamodb:UpdateItem on it.
2. CreateFunction with a Python handler written against the durable-execution programming model (@durable_execution / @durable_step from aws_durable_execution_sdk_python) and DurableConfig {ExecutionTimeout: 300, RetentionPeriodInDays: 1}. Give one step a StepConfig retry strategy of max_attempts=2 and make it raise RuntimeError on every attempt (exhaust-retries scenario) or on the first attempt only (recovery scenario).
3. Invoke the function durably with an execution name and read the DurableExecutionArn.
4. Poll GetDurableExecution(DurableExecutionArn) until Status is one of SUCCEEDED/FAILED/TIMED_OUT/STOPPED, then read Error and the execution history events.
Expected: a terminal status within seconds, with the StepFailed/StepSucceeded checkpoints described above. Actual: Status stays 'RUNNING' indefinitely and no history is produced.
Replay: the two check ids in trace_ids.
