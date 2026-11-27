Issue-Source: trace-sweep-divergence
Issue-Trace: lightsail-bucket-metric-data-request-validation
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lightsail-bucket-metric-data-request-validation lightsail-metric-and-log-stream-refusals lightsail-database-metric-and-log-request-validation lightsail-distribution-metric-data-request-validation lightsail-gui-session-and-metric-query-error-paths lightsail-instance-metric-data-input-validation lightsail-load-balancer-metric-data-refusals
Issue-Shape-Id: com.amazonaws.lightsail#Lightsail_20161128
Issue-Key: lightsail|GetRelationalDatabaseMetricData|4xx->4xx|$.message
Issue-Key: lightsail|GetInstanceMetricData|4xx->4xx|$.message
Issue-Related: 20261127T163509Z-lightsail-constraint-rejections-return-validationexception-instead-of-invalidinputexception.md
Issue-Commit: b7738e26e4fa21b25d6f40a3ea7ebe92b6b6f4d1

# Lightsail metric validation errors carry non-AWS error codes and message text

> _Note: this issue was identified against the code base at commit `b7738e26e4fa21b25d6f40a3ea7ebe92b6b6f4d1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBucketMetricData

## Description
On the Lightsail metric-query operations the emulator does raise `InvalidInputException` with HTTP 400 for cross-field and business-rule violations, but it writes its own hand-rolled `code` and message text instead of the error code and wording AWS emits for that error class. The error message and the `code` member are part of the contract, so each of these is a divergence even though the status and error type match. This bundle has 5 causes and all 5 must be closed.

Cause 1 of 5: GetBucketMetricData with startTime not earlier than endTime.
  AWS: 400 `InvalidInputException`, `code` = "InvalidInputException", `message` = "Invalid startTime and endTime, startTime must be smaller than endTime." — the entire message is fixed text; it names no request values. Smithy: GetBucketMetricData.startTime/endTime -> IsoDate, both required; errors include InvalidInputException (400). The per-operation prose itself is recorded behavior (the model documents the members, not the sentence).
  Emulator: 400 `InvalidInputException` but `code` = "InvalidParams" and `message` = "startTime must be earlier than endTime, got startTime <n> and endTime <n> (seconds since the epoch)" — its own wording, plus the two epoch values which AWS does not include.
  Why: a client switching on InvalidInputException.code sees "InvalidParams" where AWS sends "InvalidInputException", and any message assertion or user-facing surface shows text AWS never produces.
  Fix: GetBucketMetricData's validation/error construction in the lightsail service crate's ops module.
  Traces: lightsail-bucket-metric-data-request-validation#task#6, lightsail-bucket-metric-data-request-validation#task#7, lightsail-metric-and-log-stream-refusals#task#5.

Cause 2 of 5: GetRelationalDatabaseMetricData with startTime not earlier than endTime.
  AWS: 400 `InvalidInputException`, `message` = "There were <n> issues found with your request. Start time (in seconds) must be less than end time (in seconds)." — the aggregated form: fixed prefix "There were <n> issues found with your request. " where `<n>` is how many issues were found, then one fixed sentence per issue. No request values appear. Smithy: GetRelationalDatabaseMetricData.startTime/endTime -> IsoDate, required; errors include InvalidInputException (400).
  Emulator: 400 with `message` = "startTime must be earlier than endTime, got startTime <n> and endTime <n> (seconds since the epoch)" — the single-sentence hand-rolled form, not the aggregated one, and it embeds the epoch values.
  Why: the relational-database metric path reports in a different envelope from AWS, so a client cannot parse the issue count or the per-issue sentences AWS guarantees here.
  Fix: GetRelationalDatabaseMetricData's validation in the lightsail ops module.
  Traces: lightsail-database-metric-and-log-request-validation#task#4.

Cause 3 of 5: GetDistributionMetricData with a `unit` that is modelled but not published for the requested metric.
  AWS: 400 `InvalidInputException`, `code` = "InvalidInputException", `message` = "Invalid unit. Unit must be one of [<units>]." — "Invalid unit. Unit must be one of [" and "]." are fixed text; `<units>` is the set of units valid for the metric named in the request (for the Requests metric: Percent, None). Smithy: GetDistributionMetricData.unit -> MetricUnit (enum of 27 values including Bytes, so this value passes the model and must be rejected by the handler, not the constraint validator); metricName -> DistributionMetricName; errors include InvalidInputException (400).
  Emulator: 400 `InvalidInputException` but `code` = "InvalidParams" and `message` = "unit 'Bytes' is not a valid unit for the distribution metric Requests: its published unit is None" — its own wording, and it names a single published unit where AWS lists the whole valid set.
  Why: the caller is given a different code and is not told the full set of acceptable units, so it cannot repair the request from the error alone.
  Fix: GetDistributionMetricData's unit/metric compatibility check and its error construction in the lightsail ops module.
  Traces: lightsail-distribution-metric-data-request-validation#task#5, lightsail-distribution-metric-data-request-validation#task#6, lightsail-distribution-metric-data-request-validation#task#7, lightsail-metric-and-log-stream-refusals#task#3.

Cause 4 of 5: GetInstanceMetricData with an empty `statistics` list.
  AWS: 400 `InvalidInputException`, `message` = "There were <n> issues found with your request. You must provide at least 1 statistic from the set Minimum, Maximum, Sum, Average, SampleCount." — aggregated prefix as in Cause 2; the statistic set is the modelled enum. Smithy: GetInstanceMetricData.statistics -> MetricStatisticList (MetricStatistic enum values Minimum, Maximum, Sum, Average, SampleCount), required; errors include InvalidInputException (400).
  Emulator: 400 with `message` = "statistics must name at least one statistic of the metric to return" — its own wording, and it never names the valid statistic set.
  Why: the client is not told which statistics it may ask for, and the aggregated "There were N issues" envelope it expects is absent.
  Fix: GetInstanceMetricData's validation in the lightsail ops module.
  Traces: lightsail-gui-session-and-metric-query-error-paths#task#7, lightsail-gui-session-and-metric-query-error-paths#task#8, lightsail-instance-metric-data-input-validation#task#5.

Cause 5 of 5: GetLoadBalancerMetricData with startTime not earlier than endTime.
  AWS: 400 `InvalidInputException`, `message` = "There were <n> issues found with your request. Start time (in seconds) must be less than end time (in seconds)." — same aggregated wording as Cause 2, no request values.
  Emulator: 400 with `message` = "startTime must be earlier than endTime, got startTime <n> and endTime <n> (seconds since the epoch)".
  Why: as Cause 2, on the load-balancer metric path — confirming the hand-rolled window check is shared and must be replaced everywhere, not patched in one handler.
  Fix: the shared start/end-window validation used by the metric handlers in the lightsail ops module.
  Traces: lightsail-gui-session-and-metric-query-error-paths#task#11, lightsail-load-balancer-metric-data-refusals#task#4.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator, 5 divergence clusters (verdict=mismatch, service=lightsail, status 400 expected and 400 returned — only $.code/$.message diverge):

1. lightsail-bucket-metric-data-request-validation#task#6 GetBucketMetricData (affected also #task#7, lightsail-metric-and-log-stream-refusals#task#5) — $.code: expected 'InvalidInputException', got 'InvalidParams'; $.message: expected 'Invalid startTime and endTime, startTime must be smaller than endTime.', got 'startTime must be earlier than endTime, got startTime 1767312000 and endTime 1767225600 (seconds since the epoch)'.

2. lightsail-database-metric-and-log-request-validation#task#4 GetRelationalDatabaseMetricData — $.message: expected 'There were 1 issues found with your request. Start time (in seconds) must be less than end time (in seconds).', got 'startTime must be earlier than endTime, got startTime 1767312000 and endTime 1767225600 (seconds since the epoch)'.

3. lightsail-distribution-metric-data-request-validation#task#5 GetDistributionMetricData (affected also #task#6, #task#7, lightsail-metric-and-log-stream-refusals#task#3) — $.code: expected 'InvalidInputException', got 'InvalidParams'; $.message: expected 'Invalid unit. Unit must be one of [Percent, None].', got "unit 'Bytes' is not a valid unit for the distribution metric Requests: its published unit is None".

4. lightsail-gui-session-and-metric-query-error-paths#task#7 GetInstanceMetricData (affected also #task#8, lightsail-instance-metric-data-input-validation#task#5) — $.message: expected 'There were 1 issues found with your request. You must provide at least 1 statistic from the set Minimum, Maximum, Sum, Average, SampleCount.', got 'statistics must name at least one statistic of the metric to return'.

5. lightsail-gui-session-and-metric-query-error-paths#task#11 GetLoadBalancerMetricData (affected also lightsail-load-balancer-metric-data-refusals#task#4) — $.message: expected 'There were 1 issues found with your request. Start time (in seconds) must be less than end time (in seconds).', got 'startTime must be earlier than endTime, got startTime 1795200600 and endTime 1795200000 (seconds since the epoch)'.

Model check: InvalidInputException has a `code` member (string) and httpError 400; MetricUnit admits Bytes, so the unit/metric check in Cause 3 is a handler rule, not a modelled constraint; MetricStatistic = [Minimum, Maximum, Sum, Average, SampleCount].

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus.
2. Replay lightsail-bucket-metric-data-request-validation step task#6 (GetBucketMetricData, startTime > endTime) and compare $.code and $.message against the fixture.
3. Replay lightsail-database-metric-and-log-request-validation#task#4 (GetRelationalDatabaseMetricData, startTime > endTime).
4. Replay lightsail-distribution-metric-data-request-validation#task#5 (GetDistributionMetricData, unit=Bytes for metricName=Requests).
5. Replay lightsail-gui-session-and-metric-query-error-paths#task#7 (GetInstanceMetricData, statistics=[]).
6. Replay lightsail-gui-session-and-metric-query-error-paths#task#11 (GetLoadBalancerMetricData, startTime > endTime).
All five return 400 InvalidInputException, but with the emulator's own `code`/message rather than AWS's.
