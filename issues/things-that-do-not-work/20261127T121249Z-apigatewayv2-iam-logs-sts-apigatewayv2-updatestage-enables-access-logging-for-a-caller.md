Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__iam__logs__sts.access-logging-without-log-delivery-permissions-is-rejected
Issue-Key: apigatewayv2__iam__logs__sts.access-logging-without-log-delivery-permissions-is-rejected|_refusal#13|AssertionError|raise AssertionError(f"expected UpdateStage to be refused, but it returned {result!r}")
Issue-Commit: 7c686d71091806786cdd6101a8e36a3d7eadfcb8

# apigatewayv2 UpdateStage enables access logging for a caller holding no logs:CreateLogDelivery, instead of refusing it with BadRequestException

> _Note: this issue was identified against the code base at commit `7c686d71091806786cdd6101a8e36a3d7eadfcb8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStage

## Description
Cause 1 of 1: UpdateStage accepts AccessLogSettings from a principal whose identity policy does not allow `logs:CreateLogDelivery`.

AWS: recorded behavior only for the permission gate (the apigatewayv2 model declares BadRequestException on UpdateStage but states no permission precondition). Enabling access logging on a stage makes API Gateway create a CloudWatch Logs delivery on the caller's behalf, so the caller must hold `logs:CreateLogDelivery`. Without it the request is REFUSED with HTTP 400 `BadRequestException` whose message is the relayed IAM denial — fixed text "User: " + the caller's session ARN + " is not authorized to perform: logs:CreateLogDelivery because no identity-based policy allows the logs:CreateLogDelivery action". The session ARN is the request's own caller; the action name and the surrounding sentence are fixed text. The stage is left unchanged.
Emulator: HTTP 200, and the stage comes back with the AccessLogSettings applied (DestinationArn and Format echoed, LastUpdatedDate advanced). No permission check against the caller is made at all.
Why: a client that relies on this refusal to detect a mis-scoped deployment role is told logging is configured when, on AWS, the call would have failed and nothing would have been delivered. The 200 is indistinguishable from a correctly permissioned enable, so a stage silently ships with a log destination the account's own policies forbid, and the negative class (the only signal that the role is missing a grant) never appears.
Fix: the apigatewayv2 UpdateStage handler's validation/authorization step — when AccessLogSettings is present, authorize `logs:CreateLogDelivery` for the caller before persisting, and surface the denial as BadRequestException carrying the IAM message.
Traces: apigatewayv2__iam__logs__sts.access-logging-without-log-delivery-permissions-is-rejected.

## Evidence
Expected (AWS): ('BadRequestException', 'User: <caller session arn> is not authorized to perform: logs:CreateLogDelivery because no identity-based policy allows the logs:CreateLogDelivery action').

Actual (emulator), verbatim from the replay:
'AssertionError: expected UpdateStage to be refused, but it returned {ResponseMetadata: {HTTPStatusCode: 200, ...}, AccessLogSettings: {DestinationArn: arn:aws:logs:<region>:<account>:log-group:<group>, Format: {"rid": "$context.requestId", "code": "$context.status"}}, DefaultRouteSettings: {DetailedMetricsEnabled: False}, RouteSettings: {}, StageName: $default, StageVariables: {}, Tags: {...}}'

The log-group name, API id, timestamps and request id above are per-run harness values and are not part of the requirement; the requirement is the 400/BadRequestException and the message template.

## Steps to Reproduce
1. Create a CloudWatch Logs log group.
2. Create an HTTP API and its $default stage.
3. Create a role assumable by the caller whose only grants are the apigatewayv2 stage read/update actions — NO logs:CreateLogDelivery — and assume it.
4. As that session call UpdateStage(ApiId, StageName='$default', AccessLogSettings={DestinationArn: <log group arn>, Format: '<any format>'}).
5. Expect HTTP 400 BadRequestException with the logs:CreateLogDelivery denial message; observe HTTP 200 with AccessLogSettings applied.
