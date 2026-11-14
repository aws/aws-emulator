Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-connect-403-is-client-error-raising-route-execution-error
Issue-Key: apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-connect-403-is-client-error-raising-route-execution-error|run#29|AssertionError|assert lo <= total <= hi, (
Issue-Commit: 9821cff84aed55823f685374d459e562c1c72f3b

# apigatewayv2__cloudwatch__lambda: a WebSocket stage's error metrics are not mutually exclusive — a refused $connect and a raising route are each counted in more than one AWS/ApiGateway error class (1 cause)

> _Note: this issue was identified against the code base at commit `9821cff84aed55823f685374d459e562c1c72f3b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
execute-api WebSocket route dispatch (AWS/ApiGateway ClientError / ExecutionError / IntegrationError publication for a WEBSOCKET stage)

## Description
Cause 1 of 1: a WEBSOCKET stage's AWS/ApiGateway error metrics are over-counted because the emulator does not classify a data-plane failure into exactly one error class. Failing check: apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-connect-403-is-client-error-raising-route-execution-error.

AWS: recorded behavior only — CloudWatch metric semantics are not expressed in the apigatewayv2 Smithy model, so the requirement comes from the recording this check replays plus the AWS WebSocket-API metric documentation it cites (apigateway-websocket-api-logging). For a WEBSOCKET stage, each data-plane failure lands in exactly one of the {ApiId,Stage} error series: a $connect AWS_PROXY integration that returns statusCode 403 refuses the handshake and increments ClientError only; a route whose Lambda integration raises increments ExecutionError only; IntegrationError stays an explicitly published 0 for these two cases. Every series is published for an active stage (zero-valued where nothing failed). In the recorded scenario (one admitted $connect, one 403-refused $connect, one frame routed to a raising Lambda, on stage `guard`, with a clean sibling stage `live` on the same deployment) the stage totals are ClientError=1, ExecutionError=1, IntegrationError=0, and the `live` twin keeps all three at 0. Those numbers are the per-event counting rule (one increment per failing event, in one class), not values of a single run.

Emulator: the counts for the same scenario come back ClientError=1.0 (correct), ExecutionError=2.0 where exactly 1 is required, and IntegrationError=1.0 where an explicit 0 is required; the `live` twin is correct at 0/0/0. Full replayed snapshot: {'guard.ClientError': (1.0, 1), 'guard.ConnectCount': (2.0, 1), 'guard.ExecutionError': (2.0, 1), 'guard.IntegrationError': (1.0, 1), 'guard.IntegrationLatency': (3.0, 1), 'guard.MessageCount': (5.0, 1), 'live.ClientError': (0.0, 1), 'live.ConnectCount': (1.0, 1), 'live.ExecutionError': (0.0, 1), 'live.IntegrationError': (0.0, 1), 'live.IntegrationLatency': (3.0, 1), 'live.MessageCount': (3.0, 1)}. The non-error series (ConnectCount, MessageCount, IntegrationLatency) are inside their expected windows, so the stage is publishing metrics; only the error-class split is wrong. The two surplus increments are the two failing events each being recorded in a second class: the 403-refused $connect is counted as ClientError AND as an execution error, and the raising route is counted as ExecutionError AND as an integration error. No HTTP status or error code diverges here — the check reads the metrics with CloudWatch and the assertion that fires is 'guard.ExecutionError = 2.0 over 1 datapoints, expected [1, 1]'.

Why: a client that alarms on a WebSocket stage's error classes gets false alarms and the wrong diagnosis. An alarm on ExecutionError breaches at twice the real rate, so a client-side refusal (a 403 the integration deliberately returned) is reported as a server-side execution failure; an alarm on IntegrationError breaches at all, when AWS holds that series at 0 for these failures. Dashboards that split client-vs-integration-vs-execution faults to decide who is at fault therefore attribute a deliberate $connect denial to the backend.

Fix: the WEBSOCKET data-plane dispatch path of the apigatewayv2 service — the component that emits AWS/ApiGateway {ApiId,Stage} telemetry after a $connect/route integration completes. It must select exactly one error class per failing event (integration-returned 4XX on $connect -> ClientError; integration function raised / unhandled error -> ExecutionError) and keep the other error series published at 0 rather than incrementing them too. The sibling stage on the same deployment must stay unaffected.

Related but not covering this: open issue 20261113T112337Z-apigatewayv2-cloudwatch-lambda-a-websocket-api-publishes-no.md reports that a WebSocket API publishes NO AWS/ApiGateway metrics; here the series exist and carry datapoints, and only the error-class attribution diverges, so that issue does not cover this cause.

Traces: apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-connect-403-is-client-error-raising-route-execution-error (behavioral suite; the only failing check in this run not already owned or already filed).

## Evidence
Check: apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-connect-403-is-client-error-raising-route-execution-error [mismatch]

Assertion that fired:
  AssertionError: guard.ExecutionError = 2.0 over 1 datapoints, expected [1, 1] (pre-upgrade handshakes live=0 guard=0)

Expected (real-AWS recording), AWS/ApiGateway, dimensions {ApiId, Stage}:
  guard.ClientError      = 1   (the 403-refused $connect, and only that)
  guard.ExecutionError   = 1   (the raising route Lambda, and only that)
  guard.IntegrationError = 0   (published, zero-valued)
  live.ClientError / live.ExecutionError / live.IntegrationError = 0

Actual (emulator):
  guard.ClientError      = 1.0  (matches)
  guard.ExecutionError   = 2.0  (one surplus increment)
  guard.IntegrationError = 1.0  (one surplus increment; must be 0)
  live.* errors          = 0.0  (matches)

Full snapshot returned by the emulator:
  {'guard.ClientError': (1.0, 1), 'guard.ConnectCount': (2.0, 1), 'guard.ExecutionError': (2.0, 1), 'guard.IntegrationError': (1.0, 1), 'guard.IntegrationLatency': (3.0, 1), 'guard.MessageCount': (5.0, 1), 'live.ClientError': (0.0, 1), 'live.ConnectCount': (1.0, 1), 'live.ExecutionError': (0.0, 1), 'live.IntegrationError': (0.0, 1), 'live.IntegrationLatency': (3.0, 1), 'live.MessageCount': (3.0, 1)}

Note: ConnectCount, MessageCount and IntegrationLatency are all within their expected windows, so metric publication itself works; the divergence is confined to how a failing event is classified across ClientError/ExecutionError/IntegrationError.

## Steps to Reproduce
1. Create a WEBSOCKET API (CreateApi, RouteSelectionExpression "$request.body.action") and a python Lambda whose handler: returns {"statusCode": 403, "body": "denied"} for routeKey "$connect" when the stage is `guard` and the query string lacks token=ok; returns 200 for $connect/$disconnect/$default; raises RuntimeError for routeKey "boom".
2. Grant apigateway invoke permission, create an AWS_PROXY integration to the function, and routes $connect, $disconnect, $default and boom.
3. CreateDeployment, wait for it to leave PENDING, then CreateStage twice on the same deployment: `live` and `guard`.
4. On `live`: complete the RFC 6455 upgrade, send one text frame {"action":"tick"}, close cleanly.
5. On `guard`: complete an upgrade with ?token=ok (admitted). Then attempt a second handshake with no token — expect HTTP 403, no sec-websocket-accept header, body b"denied". Then on the admitted guard connection send {"action":"boom"} and read the gateway's {"message":"Internal server error"} frame, then close.
6. Read AWS/ApiGateway with dimensions {ApiId, Stage} for both stages: ConnectCount, MessageCount, IntegrationError, ClientError, ExecutionError (Sum) and IntegrationLatency (SampleCount), from the minute the scenario started.
7. Observe guard.ExecutionError = 2 (must be 1) and guard.IntegrationError = 1 (must be 0), while guard.ClientError = 1 is correct — the refused $connect and the raising route are each counted in a second error class.
