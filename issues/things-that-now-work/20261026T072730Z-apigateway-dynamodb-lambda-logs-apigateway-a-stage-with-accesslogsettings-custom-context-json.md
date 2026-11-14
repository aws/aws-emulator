Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda__logs.apigateway-access-logs-custom-format-positive
Issue-Key: apigateway__dynamodb__lambda__logs.apigateway-access-logs-custom-format-positive|_poll_log_group_json_events#18|Timeout|time.sleep(interval_s)
Issue-Commit: d7ab0aa2df0341b9bd19f61962ae23a79b6d6bd1

# apigateway: a stage with accessLogSettings (custom $context JSON format) never delivers an access-log record to the configured CloudWatch Logs group — the check hangs to the 60 s cap (1 cause)

> _Note: this issue was identified against the code base at commit `d7ab0aa2df0341b9bd19f61962ae23a79b6d6bd1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStage

## Description
Cause 1 of 1: a REST API stage configured with accessLogSettings produces no access-log record at its destination log group, and the leg that polls for one never terminates.

NOTE: the open issue 20261024T043741Z-apigateway-dynamodb-lambda-logs-three-logging-account-wiring.md lists this check id among its Issue-Trace-Ids, but its Issue-Key for this check pins a different replay frame (`_invoke#15`), so the replay suite does not report this failure as owned by it and it is filed here. A fixer should treat the two together.

AWS: Smithy (com.amazonaws.apigateway) models Stage.accessLogSettings -> AccessLogSettings{format, destinationArn}; `format` is documented as "A single line format of the access logs of data, as specified by selected $context variables. The format must include at least $context.requestId", and `destinationArn` as "The Amazon Resource Name (ARN) of the CloudWatch Logs log group or Kinesis Data Firehose delivery stream to receive access logs". With the account's cloudwatchRoleArn set to a role trusting apigateway.amazonaws.com and carrying AmazonAPIGatewayPushToCloudWatchLogs, each request served by that stage causes API Gateway to write ONE access-log event into the named log group, with every $context.* reference in the format string substituted by the value for that request — not left as the literal template text. For the format used by this check that means a JSON object whose `requestId` equals the request's x-amzn-RequestId, `httpMethod` is the request method (GET), `status` is the decimal HTTP status the stage returned ("200") and `resourcePath` is the matched resource path ("/ping"); the literal members of the format (here the `marker` value) are passed through verbatim. The fixed part of that record is the operator-supplied format string; the variable parts are taken from the request and the stage's routing. Recorded behavior only for the delivery latency; what is structural is that the request returns an HTTP status and that an access-log event appears.
Emulator: the replay reports exactly `Check timed out after 60.0s`. No access-log event matching any of the issued request ids ever appears in the destination log group (the check re-invokes on its own budget and polls DescribeLogStreams/GetLogEvents), so the whole claim is never graded — neither the substitution of $context variables nor the presence of a record. Nothing is refused either: there is no error code and no non-2xx to quote, the awaited effect simply never occurs.
Why: access logging is the primary post-deployment debugging and audit surface for a REST API. A client that enables accessLogSettings on a stage sees its UpdateStage accepted and then finds an empty log group, so it cannot tell a mis-set destinationArn or a missing account role from a stage that is serving no traffic; and because the poll never terminates, automation built on "wait for the access log record" stalls instead of failing fast.
Fix: the API Gateway stage request path and its access-log emission — the UpdateStage handler's accessLogSettings state plus the execute-api request-serving path that must render the format template and PutLogEvents into the destination log group (services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/).
Traces: apigateway__dynamodb__lambda__logs.apigateway-access-logs-custom-format-positive.

## Evidence
Expected: after update_stage(patchOperations=[replace /accessLogSettings/destinationArn, replace /accessLogSettings/format]) and a GET on the stage invoke URL that returns 200, the destination log group contains a JSON access-log event with marker="sim2real-access-log", requestId equal to the invocation's x-amzn-RequestId, httpMethod="GET", status="200", resourcePath="/ping".
Actual: `Error: Check timed out after 60.0s` — no access-log event ever appears for any of the request ids the check issued, and the poll does not terminate. No error code or non-2xx is produced by the emulator for the logging configuration itself.

## Steps to Reproduce
1. Create an IAM role trusting apigateway.amazonaws.com, attach arn:aws:iam::aws:policy/service-role/AmazonAPIGatewayPushToCloudWatchLogs, and set it with apigateway.update_account(patchOperations=[{op: replace, path: /cloudwatchRoleArn, value: <roleArn>}]).
2. logs.create_log_group(logGroupName=<group>) and build the destination ARN as arn:aws:logs:<region>:<account>:log-group:<group> (no trailing ':*').
3. Create a mock-integration REST API with a GET /ping method and create_deployment(restApiId, stageName).
4. update_stage(restApiId, stageName, patchOperations=[{replace, /accessLogSettings/destinationArn, <arn>}, {replace, /accessLogSettings/format, '{"marker":"sim2real-access-log","requestId":"$context.requestId","httpMethod":"$context.httpMethod","status":"$context.status","resourcePath":"$context.resourcePath"}'}]).
5. GET the stage invoke URL (expect 200 and an x-amzn-RequestId header), then poll the log group for a JSON event carrying that request id.
6. Observe no event ever arrives and the check exhausts the harness budget.
7. Or replay the check id in trace_ids.
