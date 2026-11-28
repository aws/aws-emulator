Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__logs.emf-log-to-metric-extraction-exactness
Issue-Key: cloudwatch__logs.emf-log-to-metric-extraction-exactness|_one#60|AssertionError|assert len(dps) == 1, (
Issue-Commit: dd656ae55f284ad82830122b966fb1260352b4e6

# cloudwatch: GetMetricStatistics at a sub-minute Period returns NO datapoints for a regular-resolution series instead of its minute-stamped bucket

> _Note: this issue was identified against the code base at commit `dd656ae55f284ad82830122b966fb1260352b4e6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
Cause 1 of 1: a CloudWatch metric stored at regular (standard) resolution is invisible to GetMetricStatistics when the request names a sub-minute Period. Failing check id: cloudwatch__logs.emf-log-to-metric-extraction-exactness.

AWS: recorded behavior only for the sub-minute read of a regular-resolution series. The Smithy model states the resolution rule but not the read outcome: GetMetricStatisticsInput$Period — "For metrics with regular resolution, a period can be as short as one minute (60 seconds) and must be a multiple of 60. For high-resolution metrics that are collected at intervals of less than one minute, the period can be 1, 5, 10, 20, 30, 60, or any multiple of 60." and the operation doc — "Data points with a period of less than 60 seconds are available for 3 hours. These data points are high-resolution metrics and are available only for custom metrics that have been defined with a StorageResolution of 1." What the recording shows is the read outcome AWS gives for the same request: a series published WITHOUT StorageResolution:1 is stored at minute granularity, and a GetMetricStatistics read of it with Period=1 still answers with that stored bucket — exactly one Datapoint, stamped on the minute boundary the samples were truncated to, aggregating every sample of that minute (Sum and SampleCount equal to the number of samples, here all 12 EMF documents of the one minute). The request succeeds (HTTP 200, no error code); the divergence is in the Datapoints list, not in a refusal. The contrast with the high-resolution twin is what the check reads: in the SAME response pair, OrderLatency (declared StorageResolution:1) at Period=1 correctly returns one bucket per second, while OrderCount (no StorageResolution declared) must return its single minute-stamped bucket.

Emulator: the Period=60 reads of the same series are correct — OrderCount under both declared dimension sets returns exactly one bucket with Sum=12, SampleCount=12, Unit=Count, and the high-resolution OrderLatency Period=1 read returns the 12 per-second buckets with the exact per-document values. Only the sub-minute read of the regular-resolution series diverges: GetMetricStatistics(Namespace=<emf namespace>, MetricName=OrderCount, Dimensions=[Service,Operation], Period=1, Statistics=[Sum,SampleCount,Minimum,Maximum]) returns Datapoints=[] — 0 datapoints where exactly 1 is required. Replay detail: "AssertionError: OrderCount under [{'Name': 'Service', 'Value': 'checkout'}, {'Name': 'Operation', 'Value': 'PlaceOrder'}] at Period=1 returned 0 datapoints, expected exactly 1: []".

Why: a client that reads a standard-resolution series at a 1-second period (dashboards and agents that use one read period for every series, and any caller that sweeps high- and standard-resolution metrics together) sees the series as absent rather than as one minute-granularity bucket. Against AWS the same call returns the data; against the emulator it reads as "no data", so alarms and comparisons built on that read conclude the metric was never published — even though the identical Period=60 read returns the full bucket. It also makes the resolution discriminator untestable: the only observable difference between a high-resolution and a standard-resolution series becomes "the standard one does not exist".

Fix: the GetMetricStatistics handler's window/retention fold — the per-datapoint retention decision that governs what a sub-minute period may see (the `granularity_available_from` / `retained_at_period` rule applied through `windows_by_unit`). A regular-resolution datum must still be folded into the window its minute-truncated timestamp falls in when the requested Period is below 60, rather than being filtered out of the series entirely; the resulting Datapoint keeps the minute-boundary Timestamp. The same rule is shared with GetMetricData's fold, so it belongs in the shared window helper rather than in the one operation.

Traces: cloudwatch__logs.emf-log-to-metric-extraction-exactness (the Period=1 read of OrderCount, after the Period=60 reads and the high-resolution Period=1 read of OrderLatency have both passed in the same check). No open issue covers this: the other cloudwatch__logs failures in this run are owned by 20261127T155721Z-cloudwatch-logs-cloudwatch-cloudwatch-logs-vended-metrics-and-contributor-insights.md (vended AWS/Logs metrics and Contributor Insights are not published at all) and by 20261031T171434Z-cloudwatch-logs-the-puttransformer-leading-parser-refusal-is.md (PutTransformer), both different causes — here extraction and publication work and only the sub-minute read of the stored series is empty.

## Evidence
Request: GetMetricStatistics Namespace=<EMF namespace>, MetricName=OrderCount, Dimensions=[{Service,checkout},{Operation,PlaceOrder}], StartTime=<minute-60>, EndTime=<now+5m>, Period=1, Statistics=[Sum,SampleCount,Minimum,Maximum].

Expected (AWS, from the recording): HTTP 200 with exactly ONE Datapoint; Timestamp == the minute boundary the 12 samples were truncated to; Sum == 12.0; SampleCount == 12.0; Unit == "Count". (12 is the number of EMF documents the check wrote in that minute — a value from the request, not a fixed part of the contract.)

Actual (emulator): HTTP 200 with Datapoints = [] — 0 datapoints.
Replay output: File "check_cloudwatch__logs_emf_log_to_metric_extraction_exactness.py", line 290, in run -> std = _one("OrderCount", d2, period=1); line 147, in _one -> AssertionError: OrderCount under [{'Name': 'Service', 'Value': 'checkout'}, {'Name': 'Operation', 'Value': 'PlaceOrder'}] at Period=1 returned 0 datapoints, expected exactly 1: []

Controls that PASSED in the same check, proving the data is stored and extraction is correct:
- GetMetricStatistics(..., Period=60) for OrderCount under both declared dimension sets: one bucket, Sum=12.0, SampleCount=12.0, Unit=Count.
- GetMetricStatistics(..., MetricName=OrderLatency, Period=1) for the series declared with StorageResolution:1: 12 one-second buckets, each SampleCount=1.0, with the per-document values.
- PutLogEvents returned no rejectedLogEventsInfo, so no log event was dropped.

## Steps to Reproduce
1. logs.CreateLogGroup + CreateLogStream, then PutLogEvents of EMF documents inside ONE whole minute, at distinct one-second offsets, whose _aws.CloudWatchMetrics declares two metrics in one envelope: one WITH "StorageResolution": 1 and one WITHOUT it (standard resolution), both under the same dimension set.
2. Wait until the extracted metrics are readable (e.g. poll GetMetricStatistics at Period=60 until the expected SampleCount appears).
3. cloudwatch.GetMetricStatistics for the STANDARD-resolution metric with Period=60 -> one bucket, Sum/SampleCount equal to the document count (passes today).
4. cloudwatch.GetMetricStatistics for the HIGH-resolution metric with Period=1 -> one bucket per second (passes today).
5. cloudwatch.GetMetricStatistics for the STANDARD-resolution metric with Period=1, same Namespace/MetricName/Dimensions/StartTime/EndTime -> emulator returns Datapoints=[]; AWS returns exactly one Datapoint stamped on the minute boundary carrying all the samples.
