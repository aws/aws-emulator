Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__logs.obs-apigatewayv2-3-websocket-access-log-connect-message-disconnect apigatewayv2__logs.obs-apigatewayv2-3-websocket-connect-rejected-logs-connect-only
Issue-Key: apigatewayv2__logs.obs-apigatewayv2-3-websocket-access-log-connect-message-disconnect|run#40|AssertionError|assert [r.get("status") for r in mine] == ["200", "200", "200"], (
Issue-Key: apigatewayv2__logs.obs-apigatewayv2-3-websocket-connect-rejected-logs-connect-only|run#37|AssertionError|assert [(r.get("eventType"), r.get("status")) for r in control[:2]] == [
Issue-Commit: 0bdb628a6eeeae829cbfa42cf4e1c8a43263b6d6

# apigatewayv2__logs: a WebSocket stage's access-log CONNECT record reports $context.status as the 101 handshake status instead of the integration's 200

> _Note: this issue was identified against the code base at commit `0bdb628a6eeeae829cbfa42cf4e1c8a43263b6d6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStage

## Description
Cause 1 of 1: the WebSocket access-log record for a successful $connect carries status '101' instead of '200'. Failing checks: apigatewayv2__logs.obs-apigatewayv2-3-websocket-access-log-connect-message-disconnect, apigatewayv2__logs.obs-apigatewayv2-3-websocket-connect-rejected-logs-connect-only.
AWS: a WebSocket stage with AccessLogSettings (Smithy: AccessLogSettings.destinationArn, AccessLogSettings.format — a $context-variable line that must include $context.requestId; no member models the status value) writes one access-log record per WebSocket event — CONNECT on $connect, MESSAGE on $default, DISCONNECT on $disconnect. Recorded behavior only for the value itself: $context.status in that record is the status of the ROUTE's response, so a connection whose $connect integration succeeds is logged with status 200 on all three records (CONNECT, MESSAGE, DISCONNECT), not the HTTP 101 of the opening handshake. The 101 is the transport's switch-protocols status and is never what $context.status expands to; a $connect integration that refuses with statusCode 403 is the case that logs a non-200 CONNECT record.
Emulator: the CONNECT record expands $context.status to the handshake status '101' while the MESSAGE record correctly carries '200'. Replay, check 1: AssertionError: every event of this connection succeeded, so each record's status must be 200, got ['101', '200', '200']. Replay, check 2 (same cause, the control connection of the deny scenario): the CONNECT record is {"...", "eventType": "CONNECT", "routeKey": "$connect", "status": "101"} where 200 is required, with the MESSAGE record at '200'. The '101' is served for every successful $connect; the per-run marker, requestId and connectionId in those records are environment values, not part of the requirement.
Why: access logs are the record operators alert and bill on. A log pipeline that counts non-2xx CONNECT records, or that distinguishes a refused handshake (403) from an accepted one (200) by $context.status, classifies every successful WebSocket connection as an anomaly in the emulator, and a dashboard or metric filter written against AWS's values silently reports no healthy connects at all.
Fix: the WEBSOCKET access-log record builder in the execute-api data plane — the value bound to the $context.status variable for the $connect event should be the route/integration response status (200 on success, the integration's refusal status when it refuses), not the 101 status line of the RFC 6455 upgrade.
Traces: apigatewayv2__logs.obs-apigatewayv2-3-websocket-access-log-connect-message-disconnect, apigatewayv2__logs.obs-apigatewayv2-3-websocket-connect-rejected-logs-connect-only.

## Evidence
apigatewayv2__logs.obs-apigatewayv2-3-websocket-access-log-connect-message-disconnect — AssertionError: every event of this connection succeeded, so each record's status must be 200, got ['101', '200', '200'].
apigatewayv2__logs.obs-apigatewayv2-3-websocket-connect-rejected-logs-connect-only — AssertionError: the control connection's CONNECT and MESSAGE must be logged with status 200, got ['{"marker": "...", "requestId": "...", "connectionId": "...", "eventType": "CONNECT", "routeKey": "$connect", "status": "101"}', '{"...", "eventType": "MESSAGE", "routeKey": "$default", "status": "200"}'] (marker/requestId/connectionId are per-run values).
Expected: status '200' on the CONNECT record of a connection whose $connect integration returned statusCode 200 (and the integration's refusal status, e.g. 403, when it refuses). Actual: '101', the opening handshake's HTTP status.

## Steps to Reproduce
1. Boot the emulator; create a CloudWatch Logs log group and set the API Gateway account CloudWatch role.
2. Create a WEBSOCKET API with one AWS_PROXY Lambda integration behind $connect, $default and $disconnect (the handler returns statusCode 200, and 200 with a body on $default).
3. CreateStage/UpdateStage with AccessLogSettings.DestinationArn = that log group and a JSON Format including $context.requestId, $context.connectionId, $context.eventType, $context.routeKey and $context.status; deploy.
4. Complete an RFC 6455 upgrade, send one text frame, close the socket, then read the stage's access-log records: AWS writes CONNECT/MESSAGE/DISCONNECT all with "status":"200"; the emulator writes "101" for CONNECT.
5. Re-replay apigatewayv2__logs.obs-apigatewayv2-3-websocket-access-log-connect-message-disconnect and apigatewayv2__logs.obs-apigatewayv2-3-websocket-connect-rejected-logs-connect-only.
