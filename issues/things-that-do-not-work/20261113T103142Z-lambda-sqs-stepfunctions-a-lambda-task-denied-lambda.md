Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sqs__stepfunctions.cli-detach-lambda-invoke-policy-breaks-sfn-task-negative lambda__sqs__stepfunctions.deploy-missing-lambda-invoke-permission-sfn-template-negative
Issue-Key: lambda__sqs__stepfunctions.cli-detach-lambda-invoke-policy-breaks-sfn-task-negative|run#54|AssertionError|assert "Status Code: 403" in cause, f"expected a 403 authorization failure, got: {cause}"
Issue-Key: lambda__sqs__stepfunctions.deploy-missing-lambda-invoke-permission-sfn-template-negative|run#20|AssertionError|assert "Status Code: 403" in cause, f"expected a 403 authorization failure, got: {cause}"
Issue-Commit: 70fd4cb3c22ab43bc53c62d82ccb3cfc2863a4f8

# lambda__sqs__stepfunctions: a Lambda Task denied lambda:InvokeFunction reports the denial with Status Code 400 and service name AWSLambda instead of Lambda / 403 (1 cause)

> _Note: this issue was identified against the code base at commit `70fd4cb3c22ab43bc53c62d82ccb3cfc2863a4f8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1: when a Step Functions Task state's execution role is not allowed lambda:InvokeFunction, the execution does fail, but the failure cause's SDK suffix misstates the refusal: it reports HTTP status 400 and the service name `AWSLambda`, where AWS reports 403 and `Lambda`. Failing checks: lambda__sqs__stepfunctions.cli-detach-lambda-invoke-policy-breaks-sfn-task-negative, lambda__sqs__stepfunctions.deploy-missing-lambda-invoke-permission-sfn-template-negative.

AWS: an IAM authorization refusal of Lambda Invoke is the common AccessDeniedException served at HTTP 403 — Smithy (lambda#Invoke, com-amazonaws-lambda-awsgirapiservice.json) models no AccessDeniedException on the operation and none of its 400-class errors (InvalidParameterValueException, InvalidRequestContentException, ...) covers an authorization denial, so the denial is the service-common 403 AccessDeniedException; the rest is recorded behavior only. Step Functions wraps that refusal into the execution's error/cause: the cause is Lambda's IAM sentence followed by the SDK suffix, `User: <assumed-role session ARN> is not authorized to perform: lambda:InvokeFunction on resource: <function ARN> because no identity-based policy allows the lambda:InvokeFunction action (Service: Lambda, Status Code: 403, Request ID: <id>)`. Fixed text: the IAM sentence template and the comma-separated `(Service: Lambda, Status Code: 403, Request ID: ...)` suffix with the service name `Lambda` and the status 403. Variable parts taken from the request/state: the assumed-role session ARN of the state machine's execution role, the function ARN, and the per-call Lambda request id.

Emulator: emits the IAM sentence correctly but closes it with `(Service: AWSLambda; Status Code: 400; Error Code: AccessDeniedException; Proxy: null)` — semicolon-separated, service name `AWSLambda`, HTTP status 400 for an authorization denial, an `Error Code`/`Proxy` pair AWS does not emit in this suffix, and no request id. Observed verbatim in both checks, e.g. `User: arn:aws:sts::<account>:assumed-role/<sfn-role>/<session> is not authorized to perform: lambda:InvokeFunction on resource: arn:aws:lambda:<region>:<account>:function:<fn> because no identity-based policy allows the lambda:InvokeFunction action (Service: AWSLambda; Status Code: 400; Error Code: AccessDeniedException; Proxy: null)`. Hypothesis: the cause is rendered by a legacy SDK-v1-style formatter instead of from the Lambda response's real status and request id.

Why: a client that parses the cause to classify the failure reads 400 (a client/validation failure) for what is actually a 403 authorization denial, and a service name that never appears in AWS causes, so retry policy, alarm filters and remediation keyed on the cause misroute the failure; the missing Request ID also removes the only handle for correlating the refusal with Lambda-side logs.

Fix: the Step Functions Lambda-integration error serializer that converts a Lambda refusal into the execution's error/cause — take status code, error code and request id from the Lambda response and render AWS's comma-separated `(Service: Lambda, Status Code: ..., Request ID: ...)` suffix. Note: open issue 20261109T210043Z-cloudformation-lambda-stepfunctions-a-step-functions-lambda-task-denied-lambda-invokefunction.md reports the same wording defect reached through a CloudFormation-deployed stack under a different service heading; it does not carry these two checks' replay keys, so they are filed here.

Traces: the two checks above; both reach the denial by removing (detach-role-policy / omitting from the template) the lambda:InvokeFunction grant on the state machine role, with a positive control proving the same wiring succeeds while the grant is present.

## Evidence
Check lambda__sqs__stepfunctions.cli-detach-lambda-invoke-policy-breaks-sfn-task-negative, line 183:
AssertionError: expected a 403 authorization failure, got: User: arn:aws:sts::000000000000:assumed-role/sim2real-detach-srole-<id>/sim2real-detach-sm-<id> is not authorized to perform: lambda:InvokeFunction on resource: arn:aws:lambda:us-west-2:000000000000:function:sim2real-detach-fn-<id> because no identity-based policy allows the lambda:InvokeFunction action (Service: AWSLambda; Status Code: 400; Error Code: AccessDeniedException; Proxy: null)

Check lambda__sqs__stepfunctions.deploy-missing-lambda-invoke-permission-sfn-template-negative, line 157: the identical suffix for a stack-deployed state machine whose role omits the grant.

Expected (AWS): ... because no identity-based policy allows the lambda:InvokeFunction action (Service: Lambda, Status Code: 403, Request ID: <id>)
Actual (emulator): ... (Service: AWSLambda; Status Code: 400; Error Code: AccessDeniedException; Proxy: null)

The role/function ARNs, session name and request id are per-run values; the divergence is the suffix format, the service name and the 400 status. The execution status (FAILED) and the error name (Lambda.AWSLambdaException) already match.

## Steps to Reproduce
1. Create a Lambda function and a state machine with a single Task state `arn:aws:states:::lambda:invoke` targeting it; grant the state machine role lambda:InvokeFunction and confirm one execution SUCCEEDS (positive control).
2. Remove the grant (detach the managed/inline policy from the role, or deploy the same stack with the grant omitted).
3. StartExecution and poll DescribeExecution until it is FAILED.
4. Read `cause`: it ends with `(Service: AWSLambda; Status Code: 400; Error Code: AccessDeniedException; Proxy: null)` instead of AWS's `(Service: Lambda, Status Code: 403, Request ID: <id>)`.
