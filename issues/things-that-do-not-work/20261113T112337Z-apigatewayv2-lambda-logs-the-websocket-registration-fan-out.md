Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__lambda__logs.registration-prunes-a-stale-ui-connection-on-gone-and-still-notifies-the-live-ui apigatewayv2__lambda__logs.registration-write-succeeds-but-no-reply-arrives-when-websocket-endpoint-is-unset
Issue-Key: apigatewayv2__lambda__logs.registration-prunes-a-stale-ui-connection-on-gone-and-still-notifies-the-live-ui|_log_messages#22|Timeout|time.sleep(4)
Issue-Key: apigatewayv2__lambda__logs.registration-write-succeeds-but-no-reply-arrives-when-websocket-endpoint-is-unset|_log_messages#18|Timeout|time.sleep(4)
Issue-Commit: 0e1ab0d3410bad8cabf8edfe5a86b78af8e5e225

# apigatewayv2__lambda__logs: the WebSocket registration/fan-out scenario never reaches a verdict — both checks exhaust the 60 s harness budget with no answer from the deployed WEBSOCKET stage

> _Note: this issue was identified against the code base at commit `0e1ab0d3410bad8cabf8edfe5a86b78af8e5e225`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
WebSocket $connect upgrade and PostToConnection fan-out

## Description
Cause 1 of 1: a deployed WEBSOCKET stage never carries the registration/fan-out session to completion, so two checks time out instead of producing a verdict. Failing checks: apigatewayv2__lambda__logs.registration-prunes-a-stale-ui-connection-on-gone-and-still-notifies-the-live-ui, apigatewayv2__lambda__logs.registration-write-succeeds-but-no-reply-arrives-when-websocket-endpoint-is-unset.

AWS: recorded behavior only (the execute-api WEBSOCKET data plane is not modelled in Smithy). On AWS the scenario runs to a verdict within the budget: an RFC 6455 upgrade to wss://<api-id>.execute-api.<region>.amazonaws.com/<stage> is answered 101 Switching Protocols with the matching Sec-WebSocket-Accept; a register/register_ui frame is dispatched by $request.body.action to the AWS_PROXY integration, which writes the connection row and replies through apigatewaymanagementapi PostToConnection; a PostToConnection to a connection whose $disconnect already ran is refused with HTTP 410 GoneException so the handler prunes that row and still fans out to the connections that remain; and with WEBSOCKET_ENDPOINT unset the handler's write succeeds while no reply frame is ever delivered. Each leg answers promptly — the handshake, the frame dispatch and the @connections call all return rather than hanging.

Emulator: both checks end with 'Check timed out after 60.0s' — the scenario never gets far enough to assert anything. No refusal, status or frame is produced that the check could grade; the session simply does not progress inside the harness budget.

Why: a client of a WebSocket-backed application hangs instead of being served or refused. Nothing times out with an error a caller can handle, no $disconnect pruning or fan-out behaviour is observable, and an application built on @connections fan-out cannot be exercised at all.

Fix: the WEBSOCKET execute-api data plane — the handshake/route-dispatch path that must answer the upgrade and dispatch frames to the integration, and the apigatewaymanagementapi @connections path that must answer PostToConnection (200, or 410 GoneException for a departed connection) instead of leaving the caller waiting.

Related open issues that do NOT cover these two checks: 20261110T153046Z-apigatewayv2-lambda-logs-the-websocket-session-scenario-and.md and 20261104T160000Z-apigatewayv2-websocket-execute-api-upgrade-remains-unservable.md.

Traces: the two failing check ids above.

## Evidence
registration-prunes-a-stale-ui-connection-on-gone-and-still-notifies-the-live-ui — Error: Check timed out after 60.0s.
registration-write-succeeds-but-no-reply-arrives-when-websocket-endpoint-is-unset — Error: Check timed out after 60.0s.

Expected (AWS): the upgrade answers 101, register/register_ui frames are answered with nodes_list / registration_success frames, a PostToConnection to a departed connection is refused HTTP 410 GoneException (the handler then logs 'Connection gone, removing from table' and deletes that row), and the live connection still receives its node_discovered notification — all inside the check budget. Actual: no verdict at all; the scenario hangs to the cap.

## Steps to Reproduce
1. Create three PAY_PER_REQUEST DynamoDB tables, an execution role, and a Lambda handler; create a WEBSOCKET API with RouteSelectionExpression $request.body.action, one AWS_PROXY integration bound to $connect/$disconnect/$default, and deploy the stage; grant execute-api:ManageConnections to the role and lambda:InvokeFunction to apigateway.amazonaws.com.
2. Open an RFC 6455 upgrade to the stage URL and send {"action":"register_ui"}, then {"action":"register", ...} from a second connection.
3. Observe that the session never produces the reply frames, the 410 GoneException prune or the fan-out notification; the check hits its 60 s cap with nothing to grade.
