Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__sns__sqs.dead-lettered-notification-counts-in-redriven-to-dlq-metric
Issue-Key: cloudwatch__sns__sqs.dead-lettered-notification-counts-in-redriven-to-dlq-metric|_await_sum#16|AssertionError|assert time.monotonic() < deadline, f"{metric} summed to {got} after {_METRIC_BUDGET_S}s, expected {total}"
Issue-Commit: 388bf0aeef5b87cdb8b8e5fe5948d049b57af517

# SNS never publishes the NumberOfNotificationsRedrivenToDlq metric for a dead-lettered notification

> _Note: this issue was identified against the code base at commit `388bf0aeef5b87cdb8b8e5fe5948d049b57af517`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
One cause.

Cause 1 of 1: a notification that SNS dead-letters is not counted in the AWS/SNS `NumberOfNotificationsRedrivenToDlq` metric.

AWS: when a subscription has a RedrivePolicy and SNS gives up delivering a notification, it moves the notification to the dead-letter queue and publishes one datapoint to the AWS/SNS namespace metric `NumberOfNotificationsRedrivenToDlq`, dimensioned by TopicName, so the metric sums to the number of dead-lettered notifications (recorded behavior only — the metric is not described by the Smithy model, which covers only the CloudWatch and SNS APIs).

Emulator: the metric sums to 0 after 420 seconds, where 1 is expected — no datapoint is ever published for the dead-lettered notification.

Why: a client's alarm or dashboard on `NumberOfNotificationsRedrivenToDlq` — the standard way of detecting that notifications are being dropped to a DLQ — never fires, so a silent delivery failure stays invisible.

Fix: the SNS dead-letter/redrive path's CloudWatch metric emission (the component that publishes AWS/SNS datapoints).

Traces: cloudwatch__sns__sqs.dead-lettered-notification-counts-in-redriven-to-dlq-metric.

## Evidence
Expected: AWS/SNS `NumberOfNotificationsRedrivenToDlq` sums to 1 for the topic after the notification is dead-lettered.
Actual: `AssertionError: NumberOfNotificationsRedrivenToDlq summed to 0 after 420.0s, expected 1`. The expected sum is derived from the number of dead-lettered notifications in the run, not a fixed value.

## Steps to Reproduce
1. Create a topic and a subscription with a RedrivePolicy naming a dead-letter queue whose endpoint cannot be delivered to.
2. Publish one message so SNS dead-letters it.
3. Poll CloudWatch GetMetricStatistics for AWS/SNS NumberOfNotificationsRedrivenToDlq with the topic's TopicName dimension — no datapoint ever appears.
Replay: cloudwatch__sns__sqs.dead-lettered-notification-counts-in-redriven-to-dlq-metric.
