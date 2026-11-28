Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__stepfunctions.put-item-parameter-path-without-the-dollar-root-rejected dynamodb__stepfunctions.put-item-task-missing-item-rejected dynamodb__stepfunctions.read-then-put-workflow-overwrites-a-change-made-between-its-read-and-write
Issue-Key: dynamodb__stepfunctions.put-item-parameter-path-without-the-dollar-root-rejected|run#20|AssertionError|assert got == ("InvalidDefinition", _REFUSED), (
Issue-Key: dynamodb__stepfunctions.put-item-task-missing-item-rejected|_refusal#19|AssertionError|raise AssertionError("a putItem Task with no Item was accepted at create")
Issue-Key: dynamodb__stepfunctions.read-then-put-workflow-overwrites-a-change-made-between-its-read-and-write|run#31|Timeout|while (desc := sfn.describe_execution(executionArn=exec_arn))["status"] == "RUNNING":
Issue-Commit: 803b8f0256cf78557f71fb793e40c4a31d070d39

# dynamodb__stepfunctions: the dynamodb:putItem Task is under-validated at CreateStateMachine and its JSONPath schema message names the wrong field path (3 causes)

> _Note: this issue was identified against the code base at commit `803b8f0256cf78557f71fb793e40c4a31d070d39`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStateMachine

## Description
Three net-new root causes in the Step Functions validation and execution of a `dynamodb:putItem` Task.

Cause 1 of 3: the SCHEMA_VALIDATION_FAILED message for a `.$` field whose value is not a JSONPath names the leaf field instead of the Parameters node, and words the rule differently.
AWS: CreateStateMachine refuses the definition with `InvalidDefinition` and the message `Invalid State Machine Definition: 'SCHEMA_VALIDATION_FAILED: The value for the field '<field>.$' must be a valid JSONPath or a valid intrinsic function call at <path>'`, where `<path>` is the `Parameters` node of the offending state (`/States/<StateName>/Parameters`). The fixed text is `must be a valid JSONPath or a valid intrinsic function call at`.
Emulator: it answers `...must be a valid JSONPath or intrinsic function expression at /States/WriteCopy/Parameters/Item/qty/N.$` — the rule is worded `or intrinsic function expression` and the path is drilled down to the leaf member rather than the Parameters node.
Why: the message is a requirement; tooling and CI that match AWS's wording or its path granularity do not recognise the refusal, and a template author is pointed at a different node than AWS reports.
Fix: the Step Functions definition validator's SCHEMA_VALIDATION_FAILED message builder for `.$` fields.
Traces: dynamodb__stepfunctions.put-item-parameter-path-without-the-dollar-root-rejected.

Cause 2 of 3: a `dynamodb:putItem` Task with no `Item` parameter is accepted at CreateStateMachine.
AWS: the required parameters of an optimized DynamoDB integration are checked at definition time; a `dynamodb:putItem` Task whose Parameters omit `Item` is refused with `InvalidDefinition` (HTTP 400) and the state machine is not created.
Emulator: CreateStateMachine succeeds — `a putItem Task with no Item was accepted at create` — deferring (or losing) the failure to run time.
Why: a broken state machine deploys green; the definition-time guard that keeps an invalid workflow out of an account does not exist.
Fix: the Step Functions definition validator's required-parameter check for optimized service-integration Tasks.
Traces: dynamodb__stepfunctions.put-item-task-missing-item-rejected.

Cause 3 of 3: a read-then-put workflow over DynamoDB never completes.
AWS: recorded behavior only — the execution runs its getItem and putItem Tasks and reaches a terminal status, overwriting a change made between its read and its write.
Emulator: the check gets no terminal execution within its budget and ends with `Check timed out after 60.0s`; no execution status or error is produced.
Why: a client polling DescribeExecution hangs on a workflow that never finishes and has no failure to act on.
Fix: the Step Functions execution engine's optimized DynamoDB integration dispatch — one of the Task legs never completes.
Traces: dynamodb__stepfunctions.read-then-put-workflow-overwrites-a-change-made-between-its-read-and-write.

## Evidence
Cause 1: got `('InvalidDefinition', "Invalid State Machine Definition: 'SCHEMA_VALIDATION_FAILED: The value for the field 'N.$' must be a valid JSONPath or intrinsic function expression at /States/WriteCopy/Parameters/Item/qty/N.$'")`, expected `InvalidDefinition "Invalid State Machine Definition: 'SCHEMA_VALIDATION_FAILED: The value for the field 'N.$' must be a valid JSONPath or a valid intrinsic function call at /States/WriteCopy/Parameters'"`.
Cause 2: `a putItem Task with no Item was accepted at create`.
Cause 3: `Check timed out after 60.0s`.

## Steps to Reproduce
1. CreateStateMachine with a dynamodb:putItem Task whose Parameters contain `Item.qty.N.$` set to a value that is not a JSONPath (no `$` root) — compare the InvalidDefinition message and path.
2. CreateStateMachine with a dynamodb:putItem Task whose Parameters omit Item — observe it is accepted.
3. Start an execution of a getItem-then-putItem state machine and poll DescribeExecution — observe it never reaches a terminal status.
