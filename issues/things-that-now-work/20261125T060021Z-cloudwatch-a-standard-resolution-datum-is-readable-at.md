Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.metric-data-high-resolution-period
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch.metric-data-high-resolution-period|run#12|AssertionError|assert std_datapoints == [], (
Issue-Commit: 5dff3459c9fa0114d27ae1b22046b841878cac27

# cloudwatch: a standard-resolution datum is readable at a sub-minute Period — GetMetricStatistics ignores the datum's StorageResolution

> _Note: this issue was identified against the code base at commit `5dff3459c9fa0114d27ae1b22046b841878cac27`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
One net-new root cause, grounded in the single failing check the suite lists under `### cloudwatch` that the replay marked neither ALREADY FILED nor OWNED (all 25 other cloudwatch failures are OWNED by existing issues). Related OPEN cloudwatch issues that do not carry this check's replay key and therefore do not cover it: 20261120T130104Z-cloudwatch-the-getmetricstatistics-1-440-datapoint-refusal-over.md (the datapoint-count refusal) and 20261120T222855Z-monitoring-putmetricdata-does-not-validate-counts-entries-so-a.md (Counts validation).

Cause 1 of 1: the storage resolution a datum was published with has no effect on retrieval granularity — a datum stored at standard (60 s) resolution is returned for a sub-minute query period, so high-resolution and regular-resolution metrics are indistinguishable. Failing check: cloudwatch.metric-data-high-resolution-period.
AWS:      Smithy: PutMetricData MetricDatum.StorageResolution — "Valid values are 1 and 60. Setting this to 1 specifies this metric as a high-resolution metric, so that CloudWatch stores the metric with sub-minute resolution down to one second. Setting this to 60 specifies this metric as a regular-resolution metric, which CloudWatch stores at 1-minute resolution. ... This field is optional, if you do not specify it the default of 60 is used." GetMetricStatisticsInput.Period — "For metrics with regular resolution, a period can be as short as one minute (60 seconds) and must be a multiple of 60. For high-resolution metrics that are collected at intervals of less than one minute, the period can be 1, 5, 10, 20, 30, 60, or any multiple of 60. High-resolution metrics are those metrics stored by a PutMetricData call that includes a StorageResolution of 1 second." So a datum published WITHOUT StorageResolution (or with 60) is stored at 1-minute resolution and a Period=1 query over its window returns an EMPTY Datapoints list, while the same datum is returned at its native Period=60; only a datum published with StorageResolution=1 yields a Period=1 datapoint. Neither query is an error — the sub-minute read of a regular-resolution metric simply has no datapoints.
Emulator: both twins behave as high resolution. In one PutMetricData call, two datums with the same namespace, same timestamp and the same value are published — one with StorageResolution=1, one with StorageResolution omitted. GetMetricStatistics at Period=1 over a +-30 s window returns a datapoint for the high-resolution metric (correct) AND one for the standard-resolution twin: `AssertionError: a standard-resolution (60s) datum must not produce a sub-minute (Period=1) datapoint the way the high-resolution twin did, got [{'Timestamp': ..., 'Sum': 42.0, 'Unit': 'Count'}]`. The expected result for that second call is `Datapoints: []`. (The timestamp and the value 42.0 in that detail are the check's own inputs for one run, not part of the requirement; the requirement is the empty list.)
Why:      a client cannot tell a high-resolution metric from a regular one, and the storage class it paid for / opted into is meaningless. Code that probes sub-minute readability to decide whether a metric supports high-resolution alarms or 1-second dashboards gets a false positive on every regular metric, so it builds Period=1/10/30 alarms and widgets over series AWS would answer empty — which then behave differently (INSUFFICIENT_DATA, empty graphs) against real AWS. Conversely, no test can verify that publishing with StorageResolution=1 actually changed anything.
Fix:      the metric-store/state component that ingests PutMetricData must record each datum's effective StorageResolution (defaulting an omitted value to 60), and the GetMetricStatistics (and GetMetricData) aggregation path must exclude regular-resolution datums from sub-minute periods, returning them only at periods that are multiples of 60, while high-resolution datums remain readable at 1, 5, 10, 20, 30, 60 and multiples of 60.
Traces:   cloudwatch.metric-data-high-resolution-period.

## Evidence
Suite replay of simparity-behavioral-aws-v1 against the emulator built from this checkout: 1144/1252 passed (91.4%), bootstrap_signal false. Under `### cloudwatch`: 685/711 (96%), 26 failed; 25 are OWNED by existing issues and this one is not.

  [mismatch] cloudwatch.metric-data-high-resolution-period
    File "check_cloudwatch_metric_data_high_resolution_period.py", line 72, in run
      assert std_datapoints == [], ...
    AssertionError: a standard-resolution (60s) datum must not produce a sub-minute (Period=1) datapoint the way the high-resolution twin did, got [{'Timestamp': datetime.datetime(2026, 11, 25, 5, 50, tzinfo=tzlocal()), 'Sum': 42.0, 'Unit': 'Count'}]

Request shape (one PutMetricData, one namespace, one instant T):
  - MetricName=HighResMetric, Timestamp=T, Value=42.0, Unit=Count, StorageResolution=1
  - MetricName=StdResMetric,  Timestamp=T, Value=42.0, Unit=Count   (StorageResolution omitted -> defaults to 60)
Reads:
  - GetMetricStatistics(HighResMetric, StartTime=T-30s, EndTime=T+30s, Period=1, Statistics=[Sum]) -> expected 1 datapoint, Sum 42.0 (emulator: correct)
  - GetMetricStatistics(StdResMetric,  StartTime=T-30s, EndTime=T+30s, Period=1, Statistics=[Sum]) -> expected HTTP 200 with Datapoints: [] (emulator: 1 datapoint, Sum 42.0) <- the divergence
  - GetMetricStatistics(StdResMetric,  StartTime=T-90s, EndTime=T+90s, Period=60, Statistics=[Sum]) -> expected 1 datapoint, Sum 42.0 (must keep working)
No error code or status is involved: both calls are 200; the divergence is that the sub-minute read of a regular-resolution metric returns a datapoint where AWS returns none. The timestamp and the value 42.0 are this run's inputs, not requirements.

## Steps to Reproduce
1. Clone the repo, boot the emulator built from that checkout, and replay simparity-behavioral-aws-v1.
2. PutMetricData into a fresh namespace with two datums at the same timestamp T and the same value: one with StorageResolution=1, one with StorageResolution omitted.
3. GetMetricStatistics for the high-resolution metric with StartTime=T-30s, EndTime=T+30s, Period=1, Statistics=['Sum'] — one datapoint is expected and returned.
4. GetMetricStatistics for the standard-resolution metric with the identical window and Period=1. Observed: a datapoint is returned. Expected: Datapoints is empty, because a datum stored at 60 s resolution is not readable at a sub-minute period.
5. GetMetricStatistics for the standard-resolution metric with StartTime=T-90s, EndTime=T+90s, Period=60 — the datum must still be returned here.
6. Re-replay cloudwatch.metric-data-high-resolution-period to confirm the fix.
