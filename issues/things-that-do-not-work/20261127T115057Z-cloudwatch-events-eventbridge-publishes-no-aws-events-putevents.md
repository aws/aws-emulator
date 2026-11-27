Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__events.put-events-requests-are-metered-by-call-entry-and-failed-entry-counts
Issue-Key: cloudwatch__events.put-events-requests-are-metered-by-call-entry-and-failed-entry-counts|_poll#9|AssertionError|raise AssertionError(f"{description} not observed within {budget_s:.0f}s; last {last!r}")
Issue-Commit: 7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8

# cloudwatch__events: EventBridge publishes no AWS/Events PutEvents metering metrics (PutEventsApproximateCallCount never appears)

> _Note: this issue was identified against the code base at commit `7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricData

## Description
Cause 1 of 1: a served PutEvents request is not metered into CloudWatch — the AWS/Events account-level metering series are never published.

AWS: each PutEvents request publishes the AWS/Events metering metrics for the account and region — PutEventsApproximateCallCount (one per API call), PutEventsApproximateEntriesCount (one per entry) and PutEventsApproximateFailedCount (one per failed entry) — so GetMetricData/GetMetricStatistics on AWS/Events returns datapoints reaching the sent counts within the normal metric delay. Recorded behavior only (the metric names are published in the EventBridge monitoring documentation, not in the Smithy model).
Emulator: the series never appears: `the AWS/Events PutEventsApproximateCallCount series reaching 1 not observed within 240s; last None` — no datapoint at all (last value None), not merely a wrong value.
Why: operators cannot observe ingestion volume or failed-entry rates, and alarms/dashboards built on the AWS/Events metering metrics stay in INSUFFICIENT_DATA forever.
Fix: the metric emission in the events PutEvents handler (it must publish the AWS/Events metering metrics into the cloudwatch metric store), i.e. the events→cloudwatch metric bridge.
Traces: cloudwatch__events.put-events-requests-are-metered-by-call-entry-and-failed-entry-counts.

## Evidence
Expected: AWS/Events PutEventsApproximateCallCount (and the entry/failed counts) datapoints reflecting the calls/entries sent.
Actual: `AssertionError: the AWS/Events PutEventsApproximateCallCount series reaching 1 not observed within 240s; last None`

## Steps to Reproduce
1. events PutEvents with a known number of entries (including one invalid entry).
2. cloudwatch GetMetricData for Namespace=AWS/Events, MetricName=PutEventsApproximateCallCount (then ...EntriesCount, ...FailedCount) with no dimensions.
3. Observe no datapoints.
