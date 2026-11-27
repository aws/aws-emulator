Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__stepfunctions.aws-sdk-lambda-invoke-async-and-response-stream-tasks-rejected lambda__stepfunctions.inputpath-that-is-not-a-reference-path-rejected lambda__stepfunctions.lambda-task-resource-naming-the-function-by-name-rejected lambda__stepfunctions.map-items-path-without-root-marker-rejected-at-create lambda__stepfunctions.parallel-state-schedules-all-three-function-branches-before-any-returns
Issue-Key: lambda__stepfunctions.aws-sdk-lambda-invoke-async-and-response-stream-tasks-rejected|_refusal#25|AssertionError|raise AssertionError(f"expected a refusal, but the call succeeded: {result!r}")
Issue-Key: lambda__stepfunctions.inputpath-that-is-not-a-reference-path-rejected|_error#23|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: lambda__stepfunctions.lambda-task-resource-naming-the-function-by-name-rejected|_refusal#11|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {answer!r}")
Issue-Key: lambda__stepfunctions.map-items-path-without-root-marker-rejected-at-create|_refusal#23|AssertionError|raise AssertionError(f"expected the definition to be refused, it was accepted: {result!r}")
Issue-Key: lambda__stepfunctions.parallel-state-schedules-all-three-function-branches-before-any-returns|run#41|AssertionError|assert max(scheduled.values()) < min(returned), (
Issue-Commit: 3c63a583c399d299d4ea298f38dfae7b708152ae

# lambda__stepfunctions: 5 net-new causes — four definition rules CreateStateMachine does not enforce, and Parallel branches are not all scheduled before any returns

> _Note: this issue was identified against the code base at commit `3c63a583c399d299d4ea298f38dfae7b708152ae`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStateMachine

## Description
Cause 1 of 5: Task Resources naming the unsupported aws-sdk Lambda operations are accepted.
AWS: CreateStateMachine refuses a definition whose Task Resource is 'arn:aws:states:::aws-sdk:lambda:invokeAsync' or 'arn:aws:states:::aws-sdk:lambda:invokeWithResponseStream' — these operations are not available through the AWS SDK integration — with InvalidDefinition (HTTP 400), message "Invalid State Machine Definition: 'SCHEMA_VALIDATION_FAILED: <reason> at /States/<state>/Resource'"; the fixed part is InvalidDefinition / 'Invalid State Machine Definition: ' / 'SCHEMA_VALIDATION_FAILED: ', and the state name and path come from the submitted definition. Smithy: CreateStateMachine errors include InvalidDefinition (client, 400).
Emulator: the call succeeds and returns a stateMachineArn.
Why: a definition AWS rejects at create is accepted here, so the failure moves to run time (or never surfaces) and a client's negative test passes wrongly.
Fix: definition validation in the CreateStateMachine handler (the aws-sdk Resource ARN allow-list).
Traces: lambda__stepfunctions.aws-sdk-lambda-invoke-async-and-response-stream-tasks-rejected.

Cause 2 of 5: an InputPath that is not a reference path is accepted.
AWS: InputPath must be a reference path; a value that is not one is refused at CreateStateMachine with InvalidDefinition "Invalid State Machine Definition: 'SCHEMA_VALIDATION_FAILED: ... at /States/<state>/InputPath'".
Emulator: the call succeeds and returns a state machine ARN.
Why: the definition is accepted and then behaves unpredictably at run time instead of being refused up front.
Fix: definition validation in CreateStateMachine (reference-path validation of InputPath).
Traces: lambda__stepfunctions.inputpath-that-is-not-a-reference-path-rejected.

Cause 3 of 5: a Lambda Task Resource that names the function by name instead of by ARN is accepted.
AWS: a Task whose Resource is 'arn:aws:states:::lambda:invoke' must carry the function's ARN in Parameters.FunctionName, and a legacy-style Resource must be a Lambda function ARN; a bare function name is refused at CreateStateMachine with InvalidDefinition and a SCHEMA_VALIDATION_FAILED message naming /States/<state>/Resource.
Emulator: the call succeeds and returns a state machine ARN.
Why: same as cause 1 — an invalid definition is admitted.
Fix: definition validation in CreateStateMachine (Resource ARN form for the Lambda integration).
Traces: lambda__stepfunctions.lambda-task-resource-naming-the-function-by-name-rejected.

Cause 4 of 5: a Map state's ItemsPath without the '$' root marker is accepted.
AWS: ItemsPath must be a reference path beginning with '$'; a value like 'items' is refused at CreateStateMachine with InvalidDefinition "Invalid State Machine Definition: 'SCHEMA_VALIDATION_FAILED: ... at /States/<state>/ItemsPath'".
Emulator: the call succeeds and returns a state machine ARN.
Why: the malformed path is accepted and silently mis-evaluated at run time.
Fix: definition validation in CreateStateMachine (reference-path validation of ItemsPath).
Traces: lambda__stepfunctions.map-items-path-without-root-marker-rejected-at-create.

Cause 5 of 5: a Parallel state does not schedule all of its branches before any branch returns.
AWS: entering a Parallel state schedules every branch's first state before any branch's task completes, so in GetExecutionHistory all the branch TaskScheduled/TaskSubmitted events precede the first branch result event.
Emulator: a branch result is recorded (event id 7) while branches are still being scheduled (scheduled event ids 5, 10, 15) — the branches are started sequentially.
Why: Parallel is not actually parallel; a workflow that relies on concurrent branch execution (fan-out latency, branches that must all be in flight to interact) behaves differently, and the event ordering a client parses out of the history is wrong.
Fix: the Parallel state executor in the Step Functions execution engine — schedule all branches, then await them.
Traces: lambda__stepfunctions.parallel-state-schedules-all-three-function-branches-before-any-returns.

## Evidence
Causes 1-4: "expected a refusal, but the call succeeded: {'stateMachineArn': 'arn:aws:states:...', ... 'HTTPStatusCode': 200 ...}" (and the variants "expected the call to be refused, but it succeeded", "the call was expected to be refused but returned ...", "expected the definition to be refused, it was accepted").
Cause 5: "a branch returned (event 7) before every branch was scheduled (scheduled event ids [5, 10, 15])".

## Steps to Reproduce
1. Call CreateStateMachine with a Task Resource 'arn:aws:states:::aws-sdk:lambda:invokeAsync' (and invokeWithResponseStream) — it is accepted (cause 1).
2. Call CreateStateMachine with a non-reference-path InputPath (cause 2), a Lambda Task Resource naming the function by name (cause 3), and a Map ItemsPath without '$' (cause 4) — all accepted.
3. Run a Parallel state with three Lambda branches and read GetExecutionHistory event ordering (cause 5).
4. Or replay the check ids in trace_ids.
