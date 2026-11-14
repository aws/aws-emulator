Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__lambda.cfn-lambda-eventinvokeconfig-maximumretryattempts-above-schema-max-rejected
Issue-Key: cloudformation__lambda.cfn-lambda-eventinvokeconfig-maximumretryattempts-above-schema-max-rejected|run#11|AssertionError|assert got == want, f"VALIDATION_ERROR events: expected {want!r}, got {got!r}"
Issue-Commit: dbbfc43f271727daf6a36bf70721e9eb6030bb1c

# cloudformation__lambda: a rolled-back AWS::Lambda::EventInvokeConfig with MaximumRetryAttempts above the schema maximum emits no VALIDATION_ERROR operation event

> _Note: this issue was identified against the code base at commit `dbbfc43f271727daf6a36bf70721e9eb6030bb1c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStackEvents

## Description
Cause 1 of 1: the stack is rolled back, but CloudFormation records no pre-provisioning property-validation event for the out-of-range property.
AWS:      Smithy (lambda): PutFunctionEventInvokeConfig's `MaximumRetryAttempts` is an integer with `range {min: 0, max: 2}`, and the AWS::Lambda::EventInvokeConfig resource schema carries the same bound. A template declaring MaximumRetryAttempts 3 fails CloudFormation's property validation BEFORE any resource is provisioned: the stack reaches ROLLBACK_COMPLETE and the stack's operation events carry one VALIDATION_ERROR event for the offending logical resource, with ValidationPath `/Resources/<LogicalId>/Properties/MaximumRetryAttempts` and ValidationStatusReason `#/MaximumRetryAttempts: 3 is not less or equal to 2` — fixed text `#/MaximumRetryAttempts: ` ... ` is not less or equal to `, with the submitted value and the schema maximum taken from the request and the schema. No resource is created.
Emulator: the stack does reach ROLLBACK_COMPLETE, but the VALIDATION_ERROR event list for the resource is EMPTY — the emulator rolls the stack back without recording the path and reason, so a client is told the stack failed but never told why or where.
Why:      a client diagnosing a failed deploy (a CI job surfacing the reason, `cdk deploy` printing the validation error, a template linter comparing against CloudFormation's own verdict) gets no diagnosis at all: no path pointing at the property and no message naming the bound that was exceeded. Tooling that keys off the VALIDATION_ERROR event type sees nothing and must fall back to guessing.
Fix:      CloudFormation's pre-provisioning property validation and the operation-event recorder: a schema range violation must emit a VALIDATION_ERROR operation event carrying LogicalResourceId, ValidationPath `/Resources/<LogicalId>/Properties/<Member>` and AWS's reason text, readable through DescribeStackEvents / the operation-events API.
Traces:   cloudformation__lambda.cfn-lambda-eventinvokeconfig-maximumretryattempts-above-schema-max-rejected.

## Evidence
Expected VALIDATION_ERROR operation events for the stack (logical id, path, reason):
  [('Eic', '/Resources/Eic/Properties/MaximumRetryAttempts', '#/MaximumRetryAttempts: 3 is not less or equal to 2')]
Actual:
  File "check_cloudformation__lambda_cfn_lambda_eventinvokeconfig_maximumretryattempts_above_schema_max_rejected.py", line 192, in run
  assert got == want, f"VALIDATION_ERROR events: expected {want!r}, got {got!r}"
  AssertionError: VALIDATION_ERROR events: expected [('Eic', '/Resources/Eic/Properties/MaximumRetryAttempts', '#/MaximumRetryAttempts: 3 is not less or equal to 2')], got []

The stack status (ROLLBACK_COMPLETE) already matches AWS; the divergence is the missing event, its ValidationPath and its reason. In the reason, `3` is the value from the request and `2` is the schema maximum; the rest of the text is fixed. The logical id 'Eic' comes from the template.

## Steps to Reproduce
1. CreateStack (Capabilities=[CAPABILITY_IAM]) with an AWS::IAM::Role for Lambda, an AWS::Lambda::Function, and an AWS::Lambda::EventInvokeConfig named `Eic` with FunctionName={Ref: Fn}, Qualifier="$LATEST" and MaximumRetryAttempts=3.
2. Poll DescribeStacks until terminal — it reaches ROLLBACK_COMPLETE.
3. Read the stack's operation events and filter EventType == VALIDATION_ERROR for LogicalResourceId 'Eic'.
4. Observe no such event, instead of one with ValidationPath '/Resources/Eic/Properties/MaximumRetryAttempts' and reason '#/MaximumRetryAttempts: 3 is not less or equal to 2'.
