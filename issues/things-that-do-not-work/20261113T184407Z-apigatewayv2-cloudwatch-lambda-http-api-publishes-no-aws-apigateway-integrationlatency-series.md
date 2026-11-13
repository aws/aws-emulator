Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__cloudwatch__lambda.metrics-elbv2-apigw-http-api-authorizer-forbidden-403-counts-lowercase-4xx-under-apiid
Issue-Key: apigatewayv2__cloudwatch__lambda.metrics-elbv2-apigw-http-api-authorizer-forbidden-403-counts-lowercase-4xx-under-apiid|_poll_until#29|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Commit: 9ca58d7cf30a6ec744c8cbd01b69502e157676e2

# HTTP API publishes no AWS/ApiGateway IntegrationLatency series under {ApiId, Stage} — the metric has no datapoints at all

> _Note: this issue was identified against the code base at commit `9ca58d7cf30a6ec744c8cbd01b69502e157676e2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
Cause 1 of 1: an HTTP API stage never publishes the AWS/ApiGateway `IntegrationLatency` metric, so the series dimensioned by (ApiId, Stage) does not exist. Failing check id: apigatewayv2__cloudwatch__lambda.metrics-elbv2-apigw-http-api-authorizer-forbidden-403-counts-lowercase-4xx-under-apiid.

AWS: CloudWatch metrics are not part of the Smithy model, so this is recorded behavior only (AWS doc: "Metrics for HTTP APIs", AWS/ApiGateway namespace). An HTTP API stage publishes `IntegrationLatency` — the time between API Gateway relaying a request to its integration and receiving the integration's response — dimensioned by (ApiId, Stage), with one sample per request that actually reached an integration. SampleCount for the window therefore equals the number of integration-reaching requests; requests the gateway refuses before the integration (for example an authorizer Deny answered 403) contribute no sample. In this scenario 13 requests reach the integration, so SampleCount settles at 13.

Emulator: GetMetricStatistics(Namespace="AWS/ApiGateway", MetricName="IntegrationLatency", Dimensions=[{Name:"ApiId",Value:<apiId>},{Name:"Stage",Value:"live"}], Statistics=["SampleCount"]) returns an empty Datapoints list for the entire 240 s budget — the harness reports `IntegrationLatency SampleCount [{'Name': 'ApiId', ...}, {'Name': 'Stage', 'Value': 'live'}] reaching 13 not met within 240s. Last result: (0, 0)`. `(0, 0)` is the harness rendering of "no datapoints / zero sum", so the series is never created, not merely under-counted. The api id, stage name and the number 13 are per-run values; the requirement is that every integration-reaching request on a stage contributes one `IntegrationLatency` sample under (ApiId, Stage).

Why: a client relying on AWS's behavior uses `IntegrationLatency` to separate backend time from gateway overhead (`Latency` minus `IntegrationLatency`), and builds p99 latency alarms, dashboards and autoscaling targets on it. With no datapoints, every such alarm stays in INSUFFICIENT_DATA forever and the client cannot tell "the backend is fast" from "the metric does not exist", so backend slowness is invisible.

Fix: the apigatewayv2 HTTP-API execute-api data plane's metric emission — the branch that relays a request to its integration must record the integration round-trip duration and publish it as an AWS/ApiGateway `IntegrationLatency` datapoint under (ApiId, Stage) into the state shared with CloudWatch GetMetricStatistics/GetMetricData, alongside the `Count`/`Latency` emission. The open issue 20261110T162302Z-apigatewayv2-cloudwatch-lambda-apigatewayv2-http-api-authorizer-forbidden-traffic-is-never.md covers the `Count`/`4xx` series for authorizer-forbidden traffic being empty; `IntegrationLatency` is a different series and is not covered by it.

Traces: apigatewayv2__cloudwatch__lambda.metrics-elbv2-apigw-http-api-authorizer-forbidden-403-counts-lowercase-4xx-under-apiid.

## Evidence
Check apigatewayv2__cloudwatch__lambda.metrics-elbv2-apigw-http-api-authorizer-forbidden-403-counts-lowercase-4xx-under-apiid, verdict mismatch:

AssertionError: IntegrationLatency SampleCount [{'Name': 'ApiId', 'Value': 'mp67t4dbd4'}, {'Name': 'Stage', 'Value': 'live'}] reaching 13 not met within 240s. Last result: (0, 0)

Expected (real AWS): after the requests that reach the AWS_PROXY integration (6 that answer 200, 3 that answer 500, and the authorizer-allowed subset), GetMetricStatistics(Namespace="AWS/ApiGateway", MetricName="IntegrationLatency", Dimensions=[{ApiId},{Stage=live}], Statistics=["SampleCount"]) reaches 13.
Actual (emulator): Datapoints is empty for the whole 240 s poll budget; the last observation is (SampleCount 0, Sum 0), i.e. the IntegrationLatency series under (ApiId, Stage) is never created.

## Steps to Reproduce
1. Create a Lambda function that serves both as a payload-2.0 REQUEST authorizer (isAuthorized from the x-sim-auth header) and as the proxy integration (500 for paths ending /boom, otherwise 200).
2. CreateApi(ProtocolType="HTTP"), CreateIntegration(IntegrationType="AWS_PROXY", PayloadFormatVersion="2.0"), routes for the ok/boom paths and an authorizer-protected route, CreateStage(StageName="live", AutoDeploy=true), and grant API Gateway lambda:InvokeFunction.
3. Send traffic through the stage so that 13 requests reach the integration (unauthenticated/authorizer-denied requests are refused 403 before the integration and must not contribute).
4. Call GetMetricStatistics(Namespace="AWS/ApiGateway", MetricName="IntegrationLatency", Dimensions=[{Name:"ApiId",Value:<apiId>},{Name:"Stage",Value:"live"}], Period=60, Statistics=["SampleCount","Sum"]) over the window: AWS reports SampleCount 13; the emulator returns an empty Datapoints list indefinitely.
