Issue-Source: trace-sweep-divergence
Issue-Trace: lightsail-bucket-metric-data-request-validation
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lightsail-bucket-metric-data-request-validation lightsail-instance-metric-data-input-validation lightsail-distribution-metric-data-request-validation lightsail-metric-and-log-stream-refusals
Issue-Shape-Id: com.amazonaws.lightsail#Lightsail_20161128
Issue-Key: lightsail|GetBucketMetricData|4xx->2xx|$.__type
Issue-Key: lightsail|GetBucketMetricData|4xx->2xx|$.code
Issue-Key: lightsail|GetBucketMetricData|4xx->2xx|$.message
Issue-Key: lightsail|GetBucketMetricData|4xx->2xx|status
Issue-Key: lightsail|StartGUISession|4xx->2xx|$.__type
Issue-Key: lightsail|StartGUISession|4xx->2xx|$.code
Issue-Key: lightsail|StartGUISession|4xx->2xx|$.message
Issue-Key: lightsail|StartGUISession|4xx->2xx|status
Issue-Key: lightsail|GetDistributionMetricData|2xx->4xx|$.metricData
Issue-Key: lightsail|GetDistributionMetricData|2xx->4xx|status
Issue-Commit: b7738e26e4fa21b25d6f40a3ea7ebe92b6b6f4d1

# Lightsail skips two input validations and over-rejects a valid distribution metric query

> _Note: this issue was identified against the code base at commit `b7738e26e4fa21b25d6f40a3ea7ebe92b6b6f4d1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBucketMetricData

## Description
Three Lightsail steps diverge on the HTTP STATUS itself: two requests AWS refuses are answered 200 by the emulator, and one request AWS answers 200 is refused with 400. This bundle has 3 causes and all 3 must be closed.

Cause 1 of 3: GetBucketMetricData asking for a statistic the bucket metrics do not publish.
  AWS: 400 `InvalidInputException`, `code` = "InvalidInputException", `message` = "Invalid statistics. Statistics can only be [<statistics>]." — "Invalid statistics. Statistics can only be [" and "]." are fixed text; `<statistics>` is the set of statistics the requested bucket metric publishes (for the bucket metrics, Average). Smithy: GetBucketMetricData.statistics -> MetricStatisticList over MetricStatistic [Minimum, Maximum, Sum, Average, SampleCount], required, and metricName -> BucketMetricName [BucketSizeBytes, NumberOfObjects]; so any of the five statistics passes the model and the narrowing to the published set is a handler rule. errors include InvalidInputException (400).
  Emulator: HTTP 200 with a metric-data body and no error members at all — the request is accepted.
  Why: a client asking for an unsupported statistic gets a 200 that implies the datapoints it receives are that statistic, when AWS would have refused the call. Code written against the emulator ships a request real AWS rejects, and any test that asserts the refusal passes vacuously.
  Fix: GetBucketMetricData's validation in the lightsail service crate's ops module — add the metric/statistics compatibility check and raise InvalidInputException.
  Traces: lightsail-bucket-metric-data-request-validation#task#4.

Cause 2 of 3: StartGUISession against an instance that is not a Lightsail for Research resource.
  AWS: 400 `InvalidInputException`, `code` = "InvalidInputException", `message` = "<resourceName> isn't a Lightsail for Research resource." — " isn't a Lightsail for Research resource." is fixed text; `<resourceName>` is the value from the request. The GUI-session operations apply only to Lightsail for Research instances, so an ordinary instance must be refused even though it exists. Smithy: StartGUISession.resourceName -> ResourceName, required; errors include InvalidInputException (400). The sentence itself is recorded behavior.
  Emulator: HTTP 200 with no error members — it starts a GUI session on an instance that cannot have one.
  Why: the emulator invents a capability. A client gets a session it could never obtain on AWS, and the refusal path that distinguishes Lightsail for Research instances from ordinary ones is never exercised.
  Fix: StartGUISession's handler/validation in the lightsail ops module — gate the operation on the resource being a Lightsail for Research instance before starting a session.
  Traces: lightsail-instance-metric-data-input-validation#task#6.

Cause 3 of 3: GetDistributionMetricData for a request AWS accepts.
  AWS: HTTP 200 with a result body whose `metricData` member is present and an empty list when the window contains no datapoints. Smithy: GetDistributionMetricDataResult.metricData -> MetricDatapointList; the operation's success code is 200. Recorded behavior: the fixture's expected response is 200 with `metricData` = [].
  Emulator: HTTP 400, and `metricData` is absent from the body — it refuses a request that is within contract.
  Why: a caller polling distribution metrics over a window with no data sees a client error instead of an empty series, so normal no-data polling is indistinguishable from a bad request and backoff/retry logic misfires.
  Fix: GetDistributionMetricData's validation in the lightsail ops module — the over-strict precondition must be removed so an in-contract query returns 200 with an empty `metricData` list rather than a 400.
  Traces: lightsail-distribution-metric-data-request-validation#task#1, lightsail-distribution-metric-data-request-validation#task#2, lightsail-metric-and-log-stream-refusals#task#4.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator, 3 divergence clusters (verdict=mismatch, service=lightsail), all with a status divergence:

1. lightsail-bucket-metric-data-request-validation#task#4 GetBucketMetricData — status: expected 400, got 200; $.__type: expected 'InvalidInputException', got None; $.code: expected 'InvalidInputException', got None; $.message: expected 'Invalid statistics. Statistics can only be [Average].', got None.

2. lightsail-instance-metric-data-input-validation#task#6 StartGUISession — status: expected 400, got 200; $.__type: expected 'InvalidInputException', got None; $.code: expected 'InvalidInputException', got None; $.message: expected "sim2real-probe-lightsail-metr-c32b1f29 isn't a Lightsail for Research resource.", got None.

3. lightsail-distribution-metric-data-request-validation#task#1 GetDistributionMetricData (affected also #task#2 and lightsail-metric-and-log-stream-refusals#task#4) — status: expected 200, got 400; $.metricData: expected [], got None.

Model check: GetBucketMetricData.metricName -> BucketMetricName [BucketSizeBytes, NumberOfObjects], statistics -> MetricStatisticList over [Minimum, Maximum, Sum, Average, SampleCount] (so the Average-only narrowing is a handler rule, not a modelled constraint); StartGUISession takes only resourceName and models InvalidInputException (400); GetDistributionMetricDataResult.metricData -> MetricDatapointList with operation success code 200.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus.
2. Replay lightsail-bucket-metric-data-request-validation step task#4 (GetBucketMetricData with a statistic other than Average): the emulator returns 200 instead of 400 InvalidInputException.
3. Replay lightsail-instance-metric-data-input-validation#task#6 (StartGUISession on an ordinary, non-Lightsail-for-Research instance): the emulator returns 200 instead of 400 InvalidInputException.
4. Replay lightsail-distribution-metric-data-request-validation#task#1 (an in-contract GetDistributionMetricData query): the emulator returns 400 instead of 200 with metricData = [].
