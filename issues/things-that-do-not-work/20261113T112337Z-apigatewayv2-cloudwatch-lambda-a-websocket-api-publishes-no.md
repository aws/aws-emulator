Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-connect-403-is-client-error-raising-route-execution-error apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-detailed-metrics-split-stage-totals-by-route apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-stage-counts-connects-frames-replies-and-pushes
Issue-Key: apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-connect-403-is-client-error-raising-route-execution-error|_poll_until#28|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-detailed-metrics-split-stage-totals-by-route|_poll_until#26|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-stage-counts-connects-frames-replies-and-pushes|_poll_until#32|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Commit: 0e1ab0d3410bad8cabf8edfe5a86b78af8e5e225

# apigatewayv2__cloudwatch__lambda: a WebSocket API publishes no AWS/ApiGateway stage (or per-route) metrics at all, so every WebSocket observability check reads zero datapoints

> _Note: this issue was identified against the code base at commit `0e1ab0d3410bad8cabf8edfe5a86b78af8e5e225`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
Cause 1 of 1: a deployed API Gateway v2 WEBSOCKET stage emits no CloudWatch metrics. Failing checks: apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-connect-403-is-client-error-raising-route-execution-error, apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-detailed-metrics-split-stage-totals-by-route, apigatewayv2__cloudwatch__lambda.obs-apigatewayv2-1-websocket-stage-counts-connects-frames-replies-and-pushes.

AWS: recorded behavior only (no Smithy shape models metric emission; the vocabulary is documented in the API Gateway developer guide, 'Monitoring WebSocket API execution with CloudWatch metrics'). A WEBSOCKET stage publishes, in namespace AWS/ApiGateway under the dimension set {ApiId, Stage}: ConnectCount, MessageCount, IntegrationLatency, ClientError, ExecutionError and IntegrationError. A $connect integration that answers a client-error status is metered as ClientError and raises ExecutionError for the route; a handler that fails is metered as ExecutionError/IntegrationError, not as a 2xx. When DetailedMetricsEnabled is set on the stage, the same counters are additionally published under {ApiId, Stage, Route} for each route key ($connect, $default and every declared route), while the stage-level series stays the sum across routes. A client reading these series with cloudwatch GetMetricStatistics/GetMetricData over the window in which the traffic happened gets non-zero Sum/SampleCount for the counters the traffic produced and zero only for the ones it did not.

Emulator: every series stays empty for the whole polling window. GetMetricStatistics answers no datapoints for any ({ApiId,Stage}) or ({ApiId,Stage,Route}) WebSocket metric: the checks poll for up to 240 s and the last observed snapshot is all zeros — e.g. expected {('live','ConnectCount'): (1,1), ('live','MessageCount'): (2,3), ('guard','ClientError'): (1,1), ('guard','ExecutionError'): (1,1), ...} vs observed {('live','ConnectCount'): (0,0), ('live','MessageCount'): (0,0), ('guard','ClientError'): (0,0), ('guard','ExecutionError'): (0,0), ...}; in the per-route check even the stage-level ('detailed', None, ...) and ('basic', None, ...) series are absent, and no per-route series exists at all. (Those tuple values are one run's traffic counts; the requirement is that the counters a WebSocket stage's traffic produces are published, not those specific numbers.)

Why: a client cannot observe or alarm on a WebSocket API. Dashboards and CloudWatch alarms over ConnectCount/MessageCount/ClientError/ExecutionError/IntegrationError never leave INSUFFICIENT_DATA, per-route attribution of a failing route is impossible, and code that decides health or scaling from these series silently sees a quiescent API while traffic is flowing and failing.

Fix: the apigatewayv2 WEBSOCKET data plane's metric emission — the stage's request/frame accounting must publish the AWS/ApiGateway counters into the emulator's CloudWatch metric store on connect, on each frame dispatch, on each integration reply and on each client/execution/integration error, with the {ApiId,Stage} dimension set always and the {ApiId,Stage,Route} set when the stage has DetailedMetricsEnabled.

Related open issue that does NOT cover these checks: 20261110T200558Z-apigatewayv2-cloudwatch-lambda-an-upgraded-websocket-connection-is.md describes an upgraded connection closed before the first frame; here the handshake and frames are reached but the metric store is never written.

Traces: the three failing check ids above.

## Evidence
obs-apigatewayv2-1-websocket-stage-counts-connects-frames-replies-and-pushes — AssertionError: WebSocket stage metrics reaching {('live','ConnectCount'): (1,1), ('live','MessageCount'): (4,5), ('live','IntegrationLatency'): (4,5), ('duplex','ConnectCount'): (1,1), ('duplex','MessageCount'): (7,8), ...} not met within 240.0s. Last result: every entry (0, 0).

obs-apigatewayv2-1-websocket-detailed-metrics-split-stage-totals-by-route — AssertionError: WebSocket stage/route metrics reaching {('detailed',None,'ConnectCount'): (1,1), ('detailed','$connect','MessageCount'): (1,1), ('detailed','boom','ExecutionError'): (1,1), ...} not met within 240.0s. Last result: only the four stage-level entries are present and all are (0, 0); no per-route entry exists.

obs-apigatewayv2-1-websocket-connect-403-is-client-error-raising-route-execution-error — AssertionError: expected ('guard','ClientError'): (1,1) and ('guard','ExecutionError'): (1,1) alongside the connect/message counters; observed all (0, 0) after 240 s.

Expected: non-zero Sum/SampleCount for the AWS/ApiGateway counters the traffic produced, under {ApiId,Stage} and, with DetailedMetricsEnabled, under {ApiId,Stage,Route}. Actual: no datapoints for any WebSocket metric.

## Steps to Reproduce
1. Create a WEBSOCKET API with RouteSelectionExpression $request.body.action and AWS_PROXY integrations for $connect/$default plus named routes; deploy a stage (one stage with DetailedMetricsEnabled, one without).
2. Complete an RFC 6455 upgrade to the stage, send several frames, provoke one integration failure and one $connect that answers a client-error status, and push one @connections message.
3. Call cloudwatch GetMetricStatistics (Sum for the counters, SampleCount for IntegrationLatency) in namespace AWS/ApiGateway for ConnectCount, MessageCount, IntegrationLatency, ClientError, ExecutionError, IntegrationError with dimensions {ApiId, Stage} and, for the detailed stage, {ApiId, Stage, Route}, over the traffic window.
4. Observe that every series is empty; AWS returns datapoints matching the traffic.
