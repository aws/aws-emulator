Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__kinesis.metrics-states-stream-iterator-age-nonzero-while-consumer-lags-and-zero-once-caught-up cloudwatch__kinesis.metrics-states-stream-write-over-one-shard-quota-rejected-and-only-accepted-records-counted
Issue-Key: cloudwatch__kinesis.metrics-states-stream-iterator-age-nonzero-while-consumer-lags-and-zero-once-caught-up|_poll#7|AssertionError|raise AssertionError(f"timed out after {_METRIC_BUDGET_S}s waiting for {what}; last {last!r}")
Issue-Key: cloudwatch__kinesis.metrics-states-stream-write-over-one-shard-quota-rejected-and-only-accepted-records-counted|_poll#21|AssertionError|raise AssertionError(f"timed out after {budget_s:.0f}s waiting for {what}; last {last!r}")
Issue-Commit: c33038d6637516191c9271b81207503148b3a194

# cloudwatch__kinesis: no AWS/Kinesis stream metric series is published — IncomingRecords, IncomingBytes, PutRecords.* and IteratorAge all stay empty after real writes

> _Note: this issue was identified against the code base at commit `c33038d6637516191c9271b81207503148b3a194`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRecords

## Description
Cause 1 of 1: writes to and reads from a Kinesis data stream mint no AWS/Kinesis metric series, so neither the throughput/throttle accounting nor the consumer-lag gauge can be observed.

AWS: a stream publishes AWS/Kinesis metrics dimensioned on {StreamName}: IncomingRecords and IncomingBytes sum the accepted writes, PutRecords.SuccessfulRecords and PutRecords.ThrottledRecords split a PutRecords batch into accepted and throttled records, WriteProvisionedThroughputExceeded counts shard-quota rejections, and GetRecords.IteratorAgeMilliseconds is non-zero while a consumer lags and returns to zero once it catches up. Only records the service ACCEPTED are counted. Recorded behavior only for the series content (these metric names are documented, not Smithy shapes); Smithy (kinesis 2013-12-02) supplies PutRecord/PutRecords/GetRecords as the data-plane contract.

Emulator: after writing, every sum stays 0.0 and ListMetrics/GetMetricStatistics return nothing — the iterator-age arm never even sees IncomingRecords reach 6 ('timed out after 600.0s waiting for IncomingRecords to reach 6; last []'), and the quota arm reads {IncomingRecords: 0.0, IncomingBytes: 0.0, PutRecords.ThrottledRecords: 0.0, PutRecords.SuccessfulRecords: 0.0} after 600 s.

Why: a Kinesis stream is unmonitorable. Shard-level throughput alarms, throttle detection and consumer-lag alarms (the standard scaling and health signals for Kinesis) can never fire, and a customer cannot tell an accepted write from a throttled one after the fact.

Fix: the Kinesis data-plane accounting component that should emit the AWS/Kinesis per-stream metric families (incoming volume, PutRecords outcome split, throughput-exceeded, GetRecords iterator age) into the CloudWatch metric store.

Traces: cloudwatch__kinesis.metrics-states-stream-iterator-age-nonzero-while-consumer-lags-and-zero-once-caught-up, cloudwatch__kinesis.metrics-states-stream-write-over-one-shard-quota-rejected-and-only-accepted-records-counted.

## Evidence
Expected: AWS/Kinesis sums reaching {'IncomingRecords': 4000.0, 'IncomingBytes': 300000.0, 'PutRecords.ThrottledRecords': 0.0, 'PutRecords.SuccessfulRecords': 4000.0} with WriteProvisionedThroughputExceeded == 0 after the accepted writes, and a non-zero GetRecords.IteratorAgeMilliseconds while the consumer lags.
Actual: 'AssertionError: timed out after 600s waiting for AWS/Kinesis stream sums ...; last {\'IncomingRecords\': 0.0, \'IncomingBytes\': 0.0, \'PutRecords.ThrottledRecords\': 0.0, \'PutRecords.SuccessfulRecords\': 0.0, \'WriteProvisionedThroughputExceeded\': 0.0}' and 'AssertionError: timed out after 600.0s waiting for IncomingRecords to reach 6; last []'.

## Steps to Reproduce
1. CreateStream with one shard.
2. PutRecords a batch (and enough volume to exceed the single-shard quota in the second arm).
3. Poll cloudwatch GetMetricStatistics for AWS/Kinesis IncomingRecords, IncomingBytes, PutRecords.SuccessfulRecords, PutRecords.ThrottledRecords, WriteProvisionedThroughputExceeded dimensioned on StreamName.
4. Start and stall a consumer and poll GetRecords.IteratorAgeMilliseconds. Observe every series is empty/zero.
