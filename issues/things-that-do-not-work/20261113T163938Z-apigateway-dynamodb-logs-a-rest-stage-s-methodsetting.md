Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__logs.deep-apigateway-1-apigw-methodsetting-logging-level-and-data-trace-gate-execution-logs
Issue-Key: apigateway__dynamodb__logs.deep-apigateway-1-apigw-methodsetting-logging-level-and-data-trace-gate-execution-logs|_until_logged#25|Timeout|fired.append(fire())
Issue-Commit: b5b438418db974c5623067148485e4df5aeb5fcd

# apigateway__dynamodb__logs: a REST stage's MethodSetting loggingLevel/dataTraceEnabled scenario never progresses — no execution logs are delivered for OFF/ERROR/INFO/data-trace

> _Note: this issue was identified against the code base at commit `b5b438418db974c5623067148485e4df5aeb5fcd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStage

## Description
Cause 1 of 1: the per-method execution-logging controls on a REST stage produce no observable behavior, so the whole OFF/ERROR/INFO/data-trace scenario stalls and is killed at the runner cap with nothing graded. Failing check: apigateway__dynamodb__logs.deep-apigateway-1-apigw-methodsetting-logging-level-and-data-trace-gate-execution-logs.

AWS: Smithy (apigateway) MethodSetting carries loggingLevel (documented: 'Specifies the logging level for this method, which affects the log entries pushed to Amazon CloudWatch Logs. Valid values are OFF, ERROR, and INFO. Choose ERROR to write only error-level entries to CloudWatch Logs, or choose INFO to include all ERROR events as well as extra informational events.') and dataTraceEnabled (@default false, 'Specifies whether data trace logging is enabled for this method'); both are set through UpdateStage patchOperations on /*/*/logging/loglevel and /*/*/logging/dataTrace and echoed by GetStage under methodSettings['*/*']. Recorded behavior: once the account's cloudwatchRoleArn names a role holding AmazonAPIGatewayPushToCloudWatchLogs, API Gateway writes execution logs to the group named API-Gateway-Execution-Logs_<rest-api-id>/<stage-name>, one stream per node, and every line is prefixed '(<requestId>)' with that request's x-amzn-RequestId. At OFF no events are delivered for either a successful or a failing request; at ERROR only the failing request is logged and only at error verbosity (the line 'Execution failed due to configuration error' appears, the INFO line 'Starting execution for request' does not, and a successful request produces nothing); at INFO the success is logged with 'Starting execution for request' and no data-trace content; with dataTraceEnabled=true the same request additionally logs 'Endpoint request body after transformations: ' with the transformed integration request, 'Endpoint response body before transformations: ' with the backend response, and 'Method response body after transformations: ' with exactly the body the client received. Those prefixes are AWS's fixed text; the request id, the marker/table name and the bodies are the request's and the state's values.

Emulator: the scenario makes no progress at all and the check is terminated at the runner's cap with 'Check timed out after 60.0s' — not a single phase (the deployed REST endpoint answering 200 on the DynamoDB-integration resource and 500 on the resource whose credentials role does not exist, then any of the three MethodSetting flips landing events in API-Gateway-Execution-Logs_<api-id>/<stage>) ever satisfies its barrier, so no execution-log event is ever observed for any logging level.

Why: loggingLevel and dataTraceEnabled are the only switches a client has for REST execution logging, and the execution log group is where an operator diagnoses a 500 from a misconfigured integration. In the emulator the switches are accepted but inert: nothing is ever written, so OFF cannot be distinguished from INFO, a configuration-error failure leaves no trace, and dataTraceEnabled reveals no request/response bodies.

Fix: the API Gateway REST data plane's execution-logging path — the per-request logger that must honour the stage's effective MethodSetting (loggingLevel OFF/ERROR/INFO and dataTraceEnabled) and deliver '(<requestId>) ...' events to API-Gateway-Execution-Logs_<api-id>/<stage> once the account cloudwatchRoleArn is set — together with the UpdateStage/GetStage methodSettings state it reads. Related open issues cover execution/access logging for other check groups but do not carry this check's replay key: 20261107T035448Z-apigateway-lambda-logs-a-rest-stage-s-access.md and 20261029T182816Z-apigateway-lambda-logs-proxy-integrations-answer-500-the.md.

Traces: the failing check id above.

## Evidence
Expected (AWS): with the account cloudwatchRoleArn set to a role holding arn:aws:iam::aws:policy/service-role/AmazonAPIGatewayPushToCloudWatchLogs, the group API-Gateway-Execution-Logs_<rest-api-id>/<stage> receives, per request id: nothing at loggingLevel OFF; only 'Execution failed due to configuration error' (and no 'Starting execution for request') for the failing request at ERROR, and nothing for a successful one; 'Starting execution for request' at INFO with no data-trace lines; and with dataTraceEnabled=true additionally exactly one 'Endpoint request body after transformations: ' line carrying the transformed integration request, one 'Endpoint response body before transformations: ' line carrying the backend response, and one 'Method response body after transformations: ' line equal to the body the client received.

Actual (emulator): 'Error: Check timed out after 60.0s'. The check never reaches any graded assertion — no phase barrier is satisfied, i.e. no execution-log event is observed in API-Gateway-Execution-Logs_<api-id>/<stage> for any of OFF, ERROR, INFO or INFO+dataTrace.

## Steps to Reproduce
1. CreateTable (PAY_PER_REQUEST, hash key pk) and create a role API Gateway may assume that allows dynamodb:UpdateItem on it.
2. CreateRestApi (REGIONAL); add GET /count with an AWS integration to arn:aws:apigateway:<region>:dynamodb:action/UpdateItem using that role and a request template that embeds $input.params('m'), plus GET /broken with the same integration but credentials naming a role that does not exist; add the 200 method/integration responses; CreateDeployment to stage 'logs'.
3. Create a role holding AmazonAPIGatewayPushToCloudWatchLogs and UpdateAccount patch /cloudwatchRoleArn to it.
4. For each setting, UpdateStage patchOperations replace /*/*/logging/loglevel = OFF | ERROR | INFO and /*/*/logging/dataTrace = false | true, confirm GetStage echoes methodSettings['*/*'], then GET the two endpoints and FilterLogEvents on API-Gateway-Execution-Logs_<api-id>/logs, indexing events by the '(<requestId>)' prefix.
Expected: the per-level delivery described above. Actual: no events at all; the scenario never progresses and the check is killed at the runner cap.
Replay: apigateway__dynamodb__logs.deep-apigateway-1-apigw-methodsetting-logging-level-and-data-trace-gate-execution-logs.
