Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__cloudwatch.http-api-count-and-4xx-metrics-carry-the-apiid-stage-dimensions
Issue-Key: apigatewayv2__cloudwatch.http-api-count-and-4xx-metrics-carry-the-apiid-stage-dimensions|run#32|AssertionError|assert count_sum <= routed_requests, (
Issue-Commit: 9ca58d7cf30a6ec744c8cbd01b69502e157676e2

# HTTP API AWS/ApiGateway Count counts route-miss requests: Count under {ApiId, Stage} exceeds the requests the stage actually routed

> _Note: this issue was identified against the code base at commit `9ca58d7cf30a6ec744c8cbd01b69502e157676e2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
Cause 1 of 1: the apigatewayv2 HTTP-API execute-api data plane increments the stage's AWS/ApiGateway `Count` series for requests that matched no route. Failing check id: apigatewayv2__cloudwatch.http-api-count-and-4xx-metrics-carry-the-apiid-stage-dimensions.

AWS: CloudWatch metrics are not part of the Smithy model, so this is recorded behavior only (AWS doc: "Metrics for HTTP APIs", AWS/ApiGateway namespace). For an HTTP API, `Count` dimensioned by (ApiId, Stage) is the number of API requests the stage ROUTED — one datapoint sample per request that resolved to a route and was handed to its integration. A request whose path matches no route key is answered by the gateway itself with 404 and the body {"message":"Not Found"}; it is not a routed API request and contributes nothing to that stage's `Count`. Consequently the sum of `Count` over the window can never exceed the number of matched-route requests a client sent to the stage, and a second, idle stage of the same API reports no datapoints at all.

Emulator: with two routes (GET /ping, GET /missing) on an auto-deployed stage, the check sent 5 requests that the integration answered 200, 3 that the integration answered 404, and 3 to a path with NO route (11 total, 9 of them matched-route). GetMetricStatistics(Namespace="AWS/ApiGateway", MetricName="Count", Dimensions=[{ApiId},{Stage}], Statistics=[Sum]) summed to 12.0. The harness reports: `AssertionError: Count for (ApiId=<apiId>, Stage=<stage>) summed to 12.0, more than the 9 matched-route requests this check made to that stage`. The counts 12/9/11 and the api id and stage name are per-run values; the requirement is the invariant — Sum(Count) must equal the matched-route request total and must exclude every gateway-produced route-miss 404.

Why: a client relying on AWS's behavior reads `Count` as the stage's served request volume and builds ratio metrics on it (5xx/Count, 4xx/Count) plus request-rate alarms and autoscaling targets. Because unrouted traffic is folded in, any 404 scanner or stray probe inflates the denominator, error-rate alarms silently under-report, and `Count` can exceed the number of requests the client knows it sent — an impossible reading that makes the series untrustworthy as a volume signal.

Fix: the apigatewayv2 execute-api data plane's HTTP-API metric emission — the point where a request's outcome is published into the CloudWatch metric store. The `Count` datapoint must be emitted only on the branch that resolved a route and invoked an integration; the gateway's own route-miss 404 branch must emit no `Count` sample for the stage. The existing open issue 20261110T162302Z-apigatewayv2-cloudwatch-apigatewayv2-http-apis-publish-no-aws-apigateway-metrics.md describes HTTP APIs publishing NO AWS/ApiGateway metrics at all; metrics now exist for this stage and over-count instead, so that issue does not cover this cause.

Traces: apigatewayv2__cloudwatch.http-api-count-and-4xx-metrics-carry-the-apiid-stage-dimensions.

## Evidence
Check apigatewayv2__cloudwatch.http-api-count-and-4xx-metrics-carry-the-apiid-stage-dimensions, verdict mismatch:

AssertionError: Count for (ApiId=mp67t4dbd4, Stage=busy) summed to 12.0, more than the 9 matched-route requests this check made to that stage — a metric cannot count traffic that did not happen, and it must not count the 3 additional requests this check sent to a path with no route

Expected (real AWS): Sum of AWS/ApiGateway `Count` for Dimensions=[{Name:ApiId},{Name:Stage}] over the window <= the matched-route requests sent (5 integration-200 + 3 integration-404 = 9), with the 3 requests to an unrouted path contributing 0 and the quiet sibling stage reporting no datapoints.
Actual (emulator): Sum == 12.0 == 9 matched-route requests + 3 unrouted route-miss requests, i.e. the gateway-produced 404 "Not Found" responses are counted into the stage's Count series.

## Steps to Reproduce
1. CreateApi(ProtocolType="HTTP"), CreateIntegration(IntegrationType="AWS_PROXY", PayloadFormatVersion="2.0") to a Lambda, and CreateRoute for RouteKey "GET /ping" and "GET /missing" targeting that integration.
2. CreateStage(StageName="busy", AutoDeploy=true) and a second CreateStage(StageName="quiet", AutoDeploy=true); grant API Gateway lambda:InvokeFunction on the execute-api ARN.
3. Send 5 GET requests to <invoke-url>/ping (integration answers 200) and 3 GET requests to <invoke-url>/missing (integration answers 404) through the "busy" stage — 9 matched-route requests total.
4. Send 3 GET requests through the same stage to a path with NO matching route key (the gateway answers 404 {"message":"Not Found"}).
5. Call GetMetricStatistics(Namespace="AWS/ApiGateway", MetricName="Count", Dimensions=[{Name:"ApiId",Value:<apiId>},{Name:"Stage",Value:"busy"}], Period=60, Statistics=["Sum"]) over the traffic window: AWS sums to 9; the emulator sums to 12, having counted the 3 route-miss requests.
