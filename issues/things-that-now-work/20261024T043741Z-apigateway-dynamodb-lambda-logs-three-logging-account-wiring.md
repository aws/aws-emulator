Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda__logs.apigateway-access-logs-custom-format-positive apigateway__dynamodb__lambda__logs.apigateway-execution-logging-to-cloudwatch-positive apigateway__dynamodb__lambda__logs.deploy-apigateway-account-cloudwatch-role-configured
Issue-Key: apigateway__dynamodb__lambda__logs.apigateway-access-logs-custom-format-positive|_invoke#15|Timeout|time.sleep(2)
Issue-Key: apigateway__dynamodb__lambda__logs.apigateway-execution-logging-to-cloudwatch-positive|_invoke#10|Timeout|time.sleep(2)
Issue-Key: apigateway__dynamodb__lambda__logs.deploy-apigateway-account-cloudwatch-role-configured|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Commit: 3ecafc116f2b44ac9fb7ddf17b83f092c778a529

# apigateway__dynamodb__lambda__logs: three logging/account-wiring checks never complete — access logs, execution logs and the CFN-deployed ApiGateway::Account check all hang to the 60 s cap (1 cause)

> _Note: this issue was identified against the code base at commit `3ecafc116f2b44ac9fb7ddf17b83f092c778a529`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStage

## Description
Cause 1 of 1: the legs these checks depend on never terminate — no stage request and no account/stack convergence poll returns.
AWS: with the account's cloudwatchRoleArn set, enabling a stage's access logging (accessLogSettings with a custom $context format) or method-level execution logging (INFO) delivers one log event per request to the configured/managed CloudWatch log group, and a CFN-deployed AWS::ApiGateway::Account resource is observable through GetAccount. Each of those steps terminates: the stage request gets an HTTP response and the account/stack read returns a value. Recorded behavior only for the log-record contents; what is structural is that each call returns.
Emulator: all three checks report exactly 'Check timed out after 60.0s' — no HTTP status line from the stage and no terminal account/stack state, so neither log delivery nor the account wiring is ever graded. Hypothesis: the dead execute-api data plane (cause 1 of 20261023T210711Z-apigateway-api-gateway-data-plane-is-dead-30-checks.md) plus the GetAccount/UpdateStage 501 stubs named in 20261023T170254Z-apigateway-dynamodb-lambda-logs-four-causes-getaccount-and.md and 20261023T210711Z-apigateway-dynamodb-lambda-logs-five-checks-hang-to.md — none of which carries these three keys.
Why: API Gateway logging is the primary debugging surface for a deployed API; a client can neither produce an access-log record nor confirm that its template actually set the account role, and the hang stalls it rather than failing it.
Fix: the execute-api data-plane request path (it must always produce a response), plus the UpdateStage/GetAccount handlers and the access-log/execution-log emission they configure (services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/).
Traces: apigateway__dynamodb__lambda__logs.apigateway-access-logs-custom-format-positive, apigateway__dynamodb__lambda__logs.apigateway-execution-logging-to-cloudwatch-positive, apigateway__dynamodb__lambda__logs.deploy-apigateway-account-cloudwatch-role-configured.

## Evidence
Error: Check timed out after 60.0s for all three checks — no HTTP status line from the deployed stage and no terminal GetAccount/stack status.

## Steps to Reproduce
1. Create an IAM role trusting apigateway.amazonaws.com with AmazonAPIGatewayPushToCloudWatchLogs and set it via update_account(replace /cloudwatchRoleArn).
2. Deploy a REST API, enable accessLogSettings with a custom format (and separately /*/*/logging/loglevel=INFO), then issue a request at the stage invoke URL and read the log group.
3. Separately, create_stack with an AWS::ApiGateway::Account resource and read apigateway.get_account().
4. Observe each flow exhausts the 60 s budget without a response.
5. Or replay the three check ids in trace_ids.
