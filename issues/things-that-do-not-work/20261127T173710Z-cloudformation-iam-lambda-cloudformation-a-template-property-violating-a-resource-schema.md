Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__lambda.durable-config-execution-timeout-ceiling-lifecycle
Issue-Key: cloudformation__iam__lambda.durable-config-execution-timeout-ceiling-lifecycle|run#18|AssertionError|assert len(validation_events) == 1, f"expected exactly one validation error event, got {validation_events!r}"
Issue-Commit: cc8c6d4514b1c20009096c9d1c08086c7dabca5f

# cloudformation: a template property violating a resource schema's numeric ceiling records no VALIDATION_ERROR operation event (AWS::Lambda::Function DurableConfig.ExecutionTimeout)

> _Note: this issue was identified against the code base at commit `cc8c6d4514b1c20009096c9d1c08086c7dabca5f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeEvents

## Description
=== Cause 1 of 1: a property-schema violation produces no VALIDATION_ERROR operation event ===
AWS: when a template property violates the resource type's schema, CloudFormation refuses it at the property — before it ever calls the underlying service — and records one operation event for the offending logical resource. `DescribeEvents` returns an `OperationEvents` entry with `EventType=VALIDATION_ERROR`, `LogicalResourceId` = the resource, `ValidationName=PROPERTY_VALIDATION`, `ValidationPath` = the JSON pointer into the template (`/Resources/<LogicalId>/Properties/<Path...>`) and `ValidationStatusReason` worded from the schema as `#/<Path>: <value> is not less or equal to <max>`. For `AWS::Lambda::Function`'s `DurableConfig.ExecutionTimeout`, whose ceiling is 31622400 seconds, a value of 31622401 yields `ValidationName=PROPERTY_VALIDATION`, `ValidationStatusReason=#/DurableConfig/ExecutionTimeout: 31622401 is not less or equal to 31622400`, `ValidationPath=/Resources/DurableFunction/Properties/DurableConfig/ExecutionTimeout`, and the stack ends `CREATE_FAILED` (OnFailure=DO_NOTHING). Fixed text: `#/`, `: `, ` is not less or equal to `; variable parts: the property pointer, the offending value from the request and the schema's ceiling. Recorded behavior only for the event's exact shape (the ceiling itself is the resource schema's).
Emulator: the check's filter over `DescribeEvents(...)["OperationEvents"]` for `EventType == "VALIDATION_ERROR"` and `LogicalResourceId == "DurableFunction"` yields `[]` — not one such event is recorded, so neither `ValidationName`, `ValidationStatusReason` nor `ValidationPath` can be read back.
Why: a client cannot learn WHICH property of WHICH resource its template got wrong. Deploy tooling and CI that surface CloudFormation's property-validation events to a developer show an empty list and report only that the stack failed.
Fix: the CloudFormation stack-operation event recorder — emit a `VALIDATION_ERROR` operation event (with `ValidationName`, `ValidationPath`, `ValidationStatusReason`) when the resource-schema property check refuses a value, and project it through `DescribeEvents`.
Traces: cloudformation__iam__lambda.durable-config-execution-timeout-ceiling-lifecycle.

## Evidence
From one replay of the behavioral suite (service heading `### cloudformation__iam__lambda`, 0/1 passed).

[mismatch] cloudformation__iam__lambda.durable-config-execution-timeout-ceiling-lifecycle
  File "check_cloudformation__iam__lambda_durable_config_execution_timeout_ceiling_lifecycle.py", line 125, in run
    assert len(validation_events) == 1, f"expected exactly one validation error event, got {validation_events!r}"
  AssertionError: expected exactly one validation error event, got []

The check filters `cfn.describe_events(StackName=<stack>)["OperationEvents"]` for `EventType == "VALIDATION_ERROR"` and `LogicalResourceId == "DurableFunction"`, then asserts:
  (ValidationName, ValidationStatusReason) == ('PROPERTY_VALIDATION', '#/DurableConfig/ExecutionTimeout: 31622401 is not less or equal to 31622400')
  ValidationPath == '/Resources/DurableFunction/Properties/DurableConfig/ExecutionTimeout'
(31622400 is the schema ceiling; 31622401 is the value the request sent.)

## Steps to Reproduce
1. Create an IAM role trusted by lambda.amazonaws.com.
2. CreateStack (OnFailure=DO_NOTHING) with one `AWS::Lambda::Function` logical id `DurableFunction` carrying `DurableConfig: {ExecutionTimeout: 31622401, RetentionPeriodInDays: 1}`.
3. Wait for the terminal state — it is CREATE_FAILED, as required.
4. Call `DescribeEvents(StackName=<stack>)` and filter `OperationEvents` for `EventType == 'VALIDATION_ERROR'` and `LogicalResourceId == 'DurableFunction'`: exactly one event is required, carrying ValidationName `PROPERTY_VALIDATION`, ValidationPath `/Resources/DurableFunction/Properties/DurableConfig/ExecutionTimeout` and ValidationStatusReason `#/DurableConfig/ExecutionTimeout: 31622401 is not less or equal to 31622400`. Observed: the filtered list is empty.
