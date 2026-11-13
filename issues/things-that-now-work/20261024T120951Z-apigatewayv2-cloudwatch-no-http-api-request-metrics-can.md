Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__cloudwatch.http-api-route-throttle-zero-rejected-429-counted-in-4xx apigatewayv2__cloudwatch.obs-apigatewayv2-2-http-api-detailed-metrics-opt-in-adds-method-resource-series
Issue-Key: apigatewayv2__cloudwatch.http-api-route-throttle-zero-rejected-429-counted-in-4xx|_poll_until#18|Timeout|time.sleep(interval_s)
Issue-Key: apigatewayv2__cloudwatch.obs-apigatewayv2-2-http-api-detailed-metrics-opt-in-adds-method-resource-series|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# apigatewayv2__cloudwatch: no HTTP-API request metrics can exist because the execute-api data plane never serves (1 cause)

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke (execute-api data plane) / GetMetricStatistics

## Description
Cause 1 of 1: an HTTP API's invoke URL never answers, so the CloudWatch series API Gateway publishes per request are never produced.
AWS: recorded behavior only (data plane + metric publication). A deployed HTTP API stage serves its invoke URL, and API Gateway publishes the AWS/ApiGatewayV2 series for that traffic: the stage-level Count/4xx/5xx totals under ApiId (and ApiId+Stage), and — once DetailedMetricsEnabled is set on the stage's route settings — the additional per-route Method/Resource series. A route whose ThrottlingRateLimit/BurstLimit is 0 answers 429 and that 429 is counted in the 4xx series.
Emulator: both checks poll the invoke URL for the API they just created and never get a servable answer, so they run past the suite's 60 s cap and are reported as `Check timed out after 60.0s`. No request is ever served, therefore no metric datum is ever emitted and the detailed-metrics opt-in and the throttled-429 counting are both unobservable.
Why: a client that validates its dashboards, alarms or throttle configuration against the emulator sees empty metric queries for traffic it believes it sent, and cannot distinguish "no traffic" from "metrics not published".
Fix: the execute-api data-plane listener for HTTP APIs (the same root absence reported for service apigatewayv2 in this sweep) and, behind it, the AWS/ApiGatewayV2 metric publication path including the DetailedMetricsEnabled per-route series.
Traces: apigatewayv2__cloudwatch.http-api-route-throttle-zero-rejected-429-counted-in-4xx, apigatewayv2__cloudwatch.obs-apigatewayv2-2-http-api-detailed-metrics-opt-in-adds-method-resource-series.

## Evidence
Expected: the stage's invoke URL answers (200 for the served route, 429 for the route throttled to 0), and GetMetricStatistics/ListMetrics over AWS/ApiGatewayV2 returns the ApiId-scoped Count/4xx series plus the Method/Resource series once detailed metrics are enabled. Actual: `Error: Check timed out after 60.0s` for both checks — the invoke poll never reaches a servable answer, so no metric is ever written.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. CreateApi(ProtocolType=HTTP) -> CreateIntegration -> CreateRoute -> CreateStage(AutoDeploy=true), optionally with RouteSettings{ThrottlingRateLimit:0} or DetailedMetricsEnabled=true.
3. GET the announced invoke URL for that api id and stage — it never answers the expected status.
4. Query CloudWatch for namespace AWS/ApiGatewayV2 with dimension ApiId=<api id> — no datapoints exist.
5. Replay apigatewayv2__cloudwatch.http-api-route-throttle-zero-rejected-429-counted-in-4xx and apigatewayv2__cloudwatch.obs-apigatewayv2-2-http-api-detailed-metrics-opt-in-adds-method-resource-series.
