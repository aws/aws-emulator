Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__stepfunctions__xray.xray-segment-tree-reflects-choice-path-lambda-status-and-retry-attempts
Issue-Key: lambda__stepfunctions__xray.xray-segment-tree-reflects-choice-path-lambda-status-and-retry-attempts|_assert_execution#12|AssertionError|assert desc["status"] == "SUCCEEDED", (
Issue-Commit: be68e03a3b164dfd55295662738d8b5c0a02662c

# lambda__stepfunctions__xray: the execution context object's State omits RetryCount, so a Task reading $$.State.RetryCount fails the execution with States.Runtime

> _Note: this issue was identified against the code base at commit `be68e03a3b164dfd55295662738d8b5c0a02662c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
REOPENS 20261110T010311Z-stepfunctions-10-net-new-causes-history-event-sequences.md: its fix left the call refused. That resolved issue's Cause 3 of 10 reported the opposite polarity of the same member — the context object carried `State.RetryCount: 0` on a Pass state that had never been retried — and its fix removed RetryCount from the context object altogether, including from the state kind where AWS does supply it. The failing check below is marked neither ALREADY FILED nor OWNED by the suite report, and no open issue carries its replay key.

Cause 1 of 1: the context object's `State` member omits RetryCount for a retried Task state, so a field whose value is `$$.State.RetryCount` cannot be resolved and the execution is failed with States.Runtime.

AWS: the Step Functions Context Object is a service-supplied JSON document addressable as `$$`; the Smithy model (com.amazonaws.sfn#AWSStepFunctions) does not model it, so this is AWS-documented behavior. AWS documents `$$.State` as `{"EnteredTime": <timestamp>, "Name": <state name>, "RetryCount": <number>}`, where RetryCount is the number of retries this state has already taken under its Retry field — 0 on the first attempt and incremented per retry. A Task state with a Retrier whose Parameters inject `"retryCount.$": "$$.State.RetryCount"` therefore resolves on every attempt, the state machine proceeds, and the execution ends SUCCEEDED (DescribeExecution status SUCCEEDED, no error/cause). A reference to a path that genuinely does not exist is what States.Runtime is for; RetryCount on a Task is not such a path.

Emulator: the context object it builds for a Task state carries only Name and EnteredTime, so the reference is unresolvable and the whole execution is refused: DescribeExecution reports status 'FAILED' with error 'States.Runtime' and cause "An error occurred while executing the state 'InvokeRetry' (entered at the event id #<id>). The JSONPath '$$.State.RetryCount' specified for the field 'retryCount.$' could not be found in the input '<the context object>'", and the dumped context object shows `"State":{"Name":"InvokeRetry","EnteredTime":"..."}` with no RetryCount beside Execution/StateMachine. In that message the fixed text is the States.Runtime wording above; the state name, event id, JSONPath, field name and the echoed document are values taken from the request and the emulator's own state, and the execution arn/role/timestamps in the recording are per-run values, not requirements.

Why: a client cannot read how many times its own Task has been retried — the single documented way to make a retried Task idempotent, to vary a backoff payload, or to tag a downstream call with its attempt number. Instead of a number it gets a hard, uncatchable-by-default States.Runtime failure at the first attempt, so the workflow never runs at all, and the X-Ray segment tree that would have recorded the retry attempts is never produced.

Fix: the context-object builder in the stepfunctions service crate (the component the earlier fix edited) — `$$.State` must carry RetryCount for state kinds that can retry (Task/Parallel/Map), starting at 0 and incremented on each retry, while continuing to omit it where AWS omits it; the JSONPath resolver that raises States.Runtime is correct and needs no change.

Traces: lambda__stepfunctions__xray.xray-segment-tree-reflects-choice-path-lambda-status-and-retry-attempts.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator built from this checkout.

[mismatch] lambda__stepfunctions__xray.xray-segment-tree-reflects-choice-path-lambda-status-and-retry-attempts
  check_...py:207 in _assert_execution -> `assert desc["status"] == "SUCCEEDED"`
  AssertionError: expected SUCCEEDED, got 'FAILED' error='States.Runtime' cause="An error occurred while executing the state 'InvokeRetry' (entered at the event id #14). The JSONPath '$$.State.RetryCount' specified for the field 'retryCount.$' could not be found in the input '{\"Execution\":{...},\"StateMachine\":{...},\"State\":{\"Name\":\"InvokeRetry\",\"EnteredTime\":\"2026-11-21T03:16:37.349Z\"}}'"

Expected (AWS): `$$.State` for the Task state `InvokeRetry` contains RetryCount alongside Name and EnteredTime, `"retryCount.$": "$$.State.RetryCount"` resolves to the attempt count (0, then 1, then 2 as its Retrier fires), and DescribeExecution ends status SUCCEEDED with no error or cause.
Actual (emulator): `$$.State` contains only Name and EnteredTime; the execution is failed with error 'States.Runtime' and the "could not be found in the input" cause above, before the check's X-Ray segment-tree assertions are reached.

Cross-check inside this same run: the sibling check stepfunctions.pass-context-object-shape-excludes-retry-count-and-task now PASSES (RetryCount correctly absent on a never-retried Pass state), confirming the member was removed unconditionally rather than per state kind.

## Steps to Reproduce
1. Build and boot the emulator from this checkout; point boto3 `stepfunctions` and `lambda` clients at it.
2. Create a Lambda function that fails on its first N invocations and succeeds afterwards.
3. Create a state machine with a Task state named `InvokeRetry` targeting that function, a `Retry` block on it, and Parameters containing `{"retryCount.$": "$$.State.RetryCount"}`.
4. StartExecution, then poll DescribeExecution.
   Observed: status 'FAILED', error 'States.Runtime', cause "... The JSONPath '$$.State.RetryCount' specified for the field 'retryCount.$' could not be found in the input '...\"State\":{\"Name\":\"InvokeRetry\",\"EnteredTime\":\"...\"}...'".
   Expected: status 'SUCCEEDED', the Task receiving retryCount 0 on its first attempt and the incremented value on each retry.
5. Full scenario: replay lambda__stepfunctions__xray.xray-segment-tree-reflects-choice-path-lambda-status-and-retry-attempts, which then asserts the X-Ray segment tree records 2 attempts for `InvokeRetry`.
