Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__firehose__s3.metric-stream-statistics-configuration-lifts-total-metric-update-while-stopped-twin-exports-nothing
Issue-Key: cloudwatch__firehose__s3.metric-stream-statistics-configuration-lifts-total-metric-update-while-stopped-twin-exports-nothing|_poll#27|Timeout|seen = observe()
Issue-Commit: 70f9d2e52da3427c1e4c43fc7da6aa78c444e3ad

# cloudwatch metric streams with a StatisticsConfiguration never converge: no exported record and no AWS/CloudWatch/MetricStreams MetricUpdate/TotalMetricUpdate within the 600 s budget

> _Note: this issue was identified against the code base at commit `70f9d2e52da3427c1e4c43fc7da6aa78c444e3ad`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricStream

## Description
Cause 1 of 1: a running metric stream that carries StatisticsConfigurations never reaches an observable exported state — neither the records Firehose should land under the destination prefix nor the stream's own AWS/CloudWatch/MetricStreams counters ever appear — so the check is killed at its 600 s ceiling with no verdict.

NOTE: the open issue 20261111T040849Z-cloudwatch-firehose-s3-cloudwatch-metric-streams-deliver-through-a-role-cloudwatch.md describes a very similar cause 2 and lists this same check in its Issue-Trace-Ids, but the replay report did NOT mark this failure as OWNED by it (the recorded replay key no longer matches), so it is filed here rather than dropped. If a fixer closes that issue's cause 2, re-replay the check named in trace_ids to confirm this one is covered too.

AWS: Smithy (cloudwatch) models PutMetricStream.StatisticsConfigurations -> MetricStreamStatisticsConfigurations (optional list of {IncludeMetrics, AdditionalStatistics}) and GetMetricStream echoes it back, plus State (MetricStreamState) and the Name/FirehoseArn/RoleArn/OutputFormat identity. The delivery and metering behaviour itself is recorded behavior only: the recording shows that, for a running stream whose IncludeFilters select a namespace that is then written with PutMetricData, (a) JSON records for that namespace land under the Firehose destination prefix within the destination's buffering interval, (b) a record from the stream that has a StatisticsConfiguration carries the configured AdditionalStatistics keys (here six beyond the default count/sum/max/min set) while a stream without one carries only the four defaults, (c) the stream publishes MetricUpdate and TotalMetricUpdate in namespace AWS/CloudWatch/MetricStreams under dimension MetricStreamName, with TotalMetricUpdate strictly greater than MetricUpdate for at least one minute (each additional statistic counts as an extra update, so the lift is bounded by MetricUpdate * (1 + number of configured additional statistics)), and (d) a twin stream stopped with StopMetricStreams before any datapoint reads back State 'stopped', exports no record and publishes no MetricUpdate, while the running control keeps exporting. All of this is observable well inside a 600 s budget.

Emulator: 'Error: Check timed out after 600.0s'. The run never leaves the poll that waits for (subject records carrying keys beyond count/sum/max/min) AND (control records) AND (a control MetricUpdate series) AND (one minute where TotalMetricUpdate > MetricUpdate), re-publishing the metric every 5 s. The earlier synchronous assertions pass (GetMetricStream echoes the StatisticsConfigurations and reports State 'running' for subject/control and 'stopped' for the twin), so the divergence is in the export/metering path, not the control-plane readback: either nothing is exported to the Firehose destination for the stream's selected namespace, or the AWS/CloudWatch/MetricStreams MetricUpdate/TotalMetricUpdate series are not published at all. Because the run is killed at the ceiling rather than asserting, the emulator also never reaches the stopped-twin absence assertions — the stopped-vs-running contract is left completely unverifiable.

Why: a client that enables additional statistics on a metric stream can never observe them take effect, can never read the stream's own update/cost counters (MetricUpdate/TotalMetricUpdate are how metric-stream volume and billing are monitored), and cannot distinguish a stopped stream from a running one by its output. Any monitoring pipeline, dashboard or cost check built on metric streams silently sees an empty destination and empty counters instead of data, and hangs waiting rather than failing.

Fix: the metric-stream export path (the publisher that fans selected metrics out to the stream's Firehose destination) plus the metering that publishes the stream's own AWS/CloudWatch/MetricStreams series: honour StatisticsConfigurations when assembling an exported record's value map, count each configured additional statistic in TotalMetricUpdate while MetricUpdate counts the metric once, publish both under dimension MetricStreamName, and make a running stream converge to delivering within its destination's buffering interval while a stopped stream exports and meters nothing.

Traces: cloudwatch__firehose__s3.metric-stream-statistics-configuration-lifts-total-metric-update-while-stopped-twin-exports-nothing.

## Evidence
Check cloudwatch__firehose__s3.metric-stream-statistics-configuration-lifts-total-metric-update-while-stopped-twin-exports-nothing [mismatch]
Error: Check timed out after 600.0s

The check's internal poll (_poll, ceiling 600 s, 5 s interval, re-publishing the metric each round) waits for: 'subject records with the additional statistics, TotalMetricUpdate > MetricUpdate on the subject, and control records and MetricUpdate'.

Expected (recorded AWS): within the budget — (1) objects under s3://<bucket>/<prefix> holding JSON metric-stream records whose metric_stream_name is the subject stream and whose value map carries the six configured additional statistics (p10, p50, p90, p99, tm90, wm90) in addition to count/sum/max/min; (2) control-stream records whose value map carries exactly {count, sum, max, min}; (3) GetMetricStatistics on Namespace=AWS/CloudWatch/MetricStreams, MetricName=MetricUpdate and MetricName=TotalMetricUpdate, Dimensions=[{MetricStreamName:<subject>}], Period=60, Statistics=[Sum] returning datapoints with TotalMetricUpdate > MetricUpdate for at least one minute and TotalMetricUpdate <= MetricUpdate * 7; (4) MetricUpdate datapoints for the control stream and none for the stopped twin, and no landed records for the stopped twin.

Actual (emulator): the readiness condition is never satisfied, so the run is killed at 600 s before any of (1)-(4) can be graded. The control-plane calls that precede the poll succeed: GetMetricStream returns State 'running' for subject and control, 'stopped' for the twin after StopMetricStreams, and echoes StatisticsConfigurations with the six AdditionalStatistics and the IncludeMetrics entry.

Note on values: the bucket name, prefix, stream names, namespace and the published value (777.0) are per-run and not part of the requirement; the requirement is the record value-map keys, the AWS/CloudWatch/MetricStreams MetricUpdate/TotalMetricUpdate series under dimension MetricStreamName, and the stopped-stream silence.

## Steps to Reproduce
1. Create an S3 bucket and a DirectPut Firehose delivery stream with ExtendedS3DestinationConfiguration (Prefix=<p>, ErrorOutputPrefix=<p>errors/, BufferingHints {SizeInMBs:1, IntervalInSeconds:0}, CompressionFormat UNCOMPRESSED); wait for DeliveryStreamStatus ACTIVE.
2. Create an IAM role trusting streams.metrics.cloudwatch.amazonaws.com with firehose:PutRecord/PutRecordBatch on the stream ARN.
3. PutMetricStream three times against that Firehose ARN/role, OutputFormat=json, IncludeFilters=[{Namespace:<ns>}]: one 'subject' with StatisticsConfigurations=[{IncludeMetrics:[{Namespace:<ns>, MetricName:StreamedMetric}], AdditionalStatistics:[p10,p50,p90,p99,tm90,wm90]}], one plain 'control', one 'stopped'.
4. StopMetricStreams(Names=[<stopped>]). GetMetricStream each: subject/control 'running' (subject echoing the StatisticsConfigurations), stopped 'stopped'. (These pass today.)
5. PutMetricData into <ns> for MetricName=StreamedMetric with two Shard dimension values; repeat every 5 s.
6. Poll ListObjectsV2/GetObject under the prefix and GetMetricStatistics(Namespace=AWS/CloudWatch/MetricStreams, MetricName=MetricUpdate|TotalMetricUpdate, Dimensions=[{MetricStreamName:<name>}], Period=60, Statistics=[Sum]). Observe: no subject record with the additional-statistics keys, no control MetricUpdate series and no minute with TotalMetricUpdate > MetricUpdate — the loop runs to its 600 s ceiling and the check is killed.
