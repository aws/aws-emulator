Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__sqs.metrics-sqs-dlq-age-of-oldest-restarts-at-the-move cloudwatch__sqs.metrics-sqs-fair-queue-quiet-group-gauges-published cloudwatch__sqs.metrics-sqs-fifo-deduplicated-sends-counted cloudwatch__sqs.metrics-sqs-fifo-groups-with-inflight-messages-gauge cloudwatch__sqs.metrics-sqs-repeated-delete-counts-every-successful-delete
Issue-Key: cloudwatch__sqs.metrics-sqs-dlq-age-of-oldest-restarts-at-the-move|_poll#15|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__sqs.metrics-sqs-fair-queue-quiet-group-gauges-published|_poll#8|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__sqs.metrics-sqs-fifo-deduplicated-sends-counted|_poll#8|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__sqs.metrics-sqs-fifo-groups-with-inflight-messages-gauge|_poll#8|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__sqs.metrics-sqs-repeated-delete-counts-every-successful-delete|_poll#9|Timeout|time.sleep(interval_s)
Issue-Commit: 388bf0aeef5b87cdb8b8e5fe5948d049b57af517

# SQS publishes none of the AWS/SQS CloudWatch metric series the queue's data plane is required to emit

> _Note: this issue was identified against the code base at commit `388bf0aeef5b87cdb8b8e5fe5948d049b57af517`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
One cause.

Cause 1 of 1: the SQS data plane emits no AWS/SQS CloudWatch datapoints, so every metric-shaped observation on a queue never materializes.

AWS: recorded behavior only (these metrics are not in the Smithy model, which covers the SQS and CloudWatch APIs). SQS publishes AWS/SQS namespace metrics dimensioned by QueueName at a fixed interval and in response to data-plane activity: `ApproximateAgeOfOldestMessage` (which restarts from the move when a message is redriven into a dead-letter queue), the fair-queue group gauges for a quiet message group, `NumberOfMessagesSent` not counting a FIFO send suppressed by deduplication, the gauge of FIFO message groups holding in-flight messages, and `NumberOfMessagesDeleted` counting every successful DeleteMessage including repeats of the same handle.

Emulator: none of these series ever appears. All five checks reach their deadline waiting for the first datapoint (three at the 600 s budget, two at the 60 s budget) without the metric being observable at all.

Why: a client's autoscaling policy, alarm or dashboard built on AWS/SQS metrics — the normal way to drive consumers and detect backlogs, dedup suppression and stuck groups — never sees data. Queue depth and age look permanently absent rather than zero, so scaling and alerting silently do nothing.

Fix: the SQS queue state/data-plane metric emitter that must publish AWS/SQS datapoints into CloudWatch (send, receive, delete, redrive and the periodic gauge sampler).

Traces: the five cloudwatch__sqs.metrics-sqs-* check ids listed in trace_ids.

## Evidence
Expected: the named AWS/SQS metric series is observable for the queue's QueueName dimension within the check's polling window and carries the values the operations performed imply.
Actual (suite report): every check ends in its own budget with no datapoint —
- metrics-sqs-dlq-age-of-oldest-restarts-at-the-move: `Check timed out after 600.0s`
- metrics-sqs-fair-queue-quiet-group-gauges-published: `Check timed out after 600.0s`
- metrics-sqs-fifo-groups-with-inflight-messages-gauge: `Check timed out after 600.0s`
- metrics-sqs-fifo-deduplicated-sends-counted: `Check timed out after 60.0s`
- metrics-sqs-repeated-delete-counts-every-successful-delete: `Check timed out after 60.0s`
The expected datapoint values are derived from the operations each check performs, not from a recorded literal.

## Steps to Reproduce
1. Create an SQS queue (standard, FIFO, or a queue with a dead-letter target, per the individual check).
2. Drive the data plane: send, receive, delete, redrive, or let a FIFO group go quiet.
3. Poll CloudWatch for the corresponding AWS/SQS metric with the QueueName dimension — no datapoint is ever returned.
Replay: the five check ids in trace_ids.
