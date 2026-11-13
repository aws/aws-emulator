Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudformation__lambda.apigw-integration-lambda-alias-mismatch-negative apigateway__cloudformation__lambda.cfn-lambda-role-trust-policy-positive
Issue-Key: apigateway__cloudformation__lambda.apigw-integration-lambda-alias-mismatch-negative|run#27|AssertionError|assert "Invalid permissions on Lambda function" in bad_resp.get("log", ""), (
Issue-Key: apigateway__cloudformation__lambda.cfn-lambda-role-trust-policy-positive|_poll_until#13|Timeout|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Commit: ad889ea564c74ff66a8d5d7080e6fcd3e62d9684

# apigateway__cloudformation__lambda: 2 net-new causes — TestInvokeMethod invents the configuration-error log line for an unresolvable Lambda alias, and the CFN Lambda-role trust check never settles

> _Note: this issue was identified against the code base at commit `ad889ea564c74ff66a8d5d7080e6fcd3e62d9684`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TestInvokeMethod

## Description
Two distinct root causes. A fixer must close both before submitting.

Cause 1 of 2: TestInvokeMethod's execution log for an AWS_PROXY integration whose URI names a Lambda alias that does not exist.
AWS: status 500 and the `log` member (Smithy com.amazonaws.apigateway#TestInvokeMethod output: status/body/headers/multiValueHeaders/log/latency) carries the configuration-error line 'Execution failed due to configuration error: Invalid permissions on Lambda function' — API Gateway reports an unresolvable function qualifier as a PERMISSIONS error, not a not-found error. The fixed text of that line is 'Execution failed due to configuration error: Invalid permissions on Lambda function'; the rest of the log's lines carry per-run timestamps and the test-invoke request id, which are not requirements.
Emulator: the status is right (500) and the function is correctly never run, but the log line reads 'Execution failed due to configuration error: the AWS_PROXY integration's function did not answer the invocation, so there is no function answer to pass through' — prose AWS never emits.
Why: the execution log is the only diagnostic TestInvokeMethod gives; a client or operator that keys on it to separate a permissions/qualifier misconfiguration from a backend timeout matches nothing.
Fix: the TestInvokeMethod handler's execution-log composition for an AWS_PROXY integration whose Lambda qualifier cannot be resolved.
Traces: apigateway__cloudformation__lambda.apigw-integration-lambda-alias-mismatch-negative (the sibling method on the real alias returns 200 with the invoked marker, so the integration path itself works).

Cause 2 of 2: a stack whose IAM role trusts lambda.amazonaws.com and whose function runs under it never settles inside the check budget.
AWS: the stack reaches CREATE_COMPLETE, Invoke on the stack's FunctionArn returns StatusCode 200 with the echoed payload ({'ok': true, 'received': {...}}) and no FunctionError, and because the role grants logs:CreateLogGroup/PutLogEvents the invocation produces a /aws/lambda/<function> stream whose lastEventTimestamp is set — all well inside a minute.
Emulator: recorded behavior only — no progress is made and the harness reports 'Check timed out after 60.0s', so no verdict on the assumed-role path is ever reached. Hypothesis: either the stack never reaches a terminal status or the invoked function never writes a CloudWatch Logs stream with events, so the poll never satisfies.
Why: a client that deploys a function with a stack-created execution role and then waits for either the stack or the function's logs waits forever.
Fix: whichever of the CloudFormation stack progression or the Lambda runtime's log-stream emission the poll depends on must reach a terminal state.
Traces: apigateway__cloudformation__lambda.cfn-lambda-role-trust-policy-positive. Open issue 20261105T111951Z-apigateway-cloudformation-lambda-the-lambda-python-runtime-has.md lists this check id under its Cause 2, but the suite no longer matches it to that issue's replay key; this filing records the current key.

## Evidence
Cause 1 — expected: bad_resp['log'] contains 'Invalid permissions on Lambda function'. Actual: log='Execution log for request test-invoke-request ... : Integration type: AWS_PROXY ... : Execution failed due to configuration error: the AWS_PROXY integration's function did not answer the invocation, so there is no function answer to pass through ... : Answered through this API's API_CONFIGURATION_ERROR gateway response ... : Method completed with status: 500'.
Cause 2 — expected: stack CREATE_COMPLETE, Invoke 200 with payload {'ok': True, 'received': {'ping': 'pong'}}, and a log stream with events. Actual: 'Error: Check timed out after 60.0s'.

## Steps to Reproduce
Cause 1: publish a Lambda version and a 'live' alias; create a REST API with two GET methods, one AWS_PROXY integration whose URI ends ':live/invocations' and one whose URI ends ':sim2real-missing-alias/invocations'; call TestInvokeMethod on both and compare status and `log`. Cause 2: CreateStack with an AWS::IAM::Role trusting lambda.amazonaws.com (AWSLambdaBasicExecutionRole attached) plus an AWS::Lambda::Function using Fn::GetAtt role Arn, wait for CREATE_COMPLETE, Invoke the function and poll logs:DescribeLogStreams for /aws/lambda/<function>.
