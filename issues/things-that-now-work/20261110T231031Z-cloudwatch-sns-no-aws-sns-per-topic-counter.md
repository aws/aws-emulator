Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__sns.filtered-out-and-failed-notification-metrics-count-exactly-the-withheld-and-refused-publishes
Issue-Key: cloudwatch__sns.filtered-out-and-failed-notification-metrics-count-exactly-the-withheld-and-refused-publishes|_poll_until#25|AssertionError|raise AssertionError(f"{description} did not happen within {timeout_s}s; last reading {last!r}")
Issue-Commit: cd6d73564e5f4cf53934bca1327f08de31f2ee5f

# cloudwatch__sns: no AWS/SNS per-topic counter is ever published, so NumberOfMessagesPublished / NumberOfNotificationsFilteredOut / NumberOfNotificationsFailed cannot be read

> _Note: this issue was identified against the code base at commit `cd6d73564e5f4cf53934bca1327f08de31f2ee5f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
Cause 1 of 1: publishing to a topic emits no AWS/SNS metric data at all, so the three counters that let a client tell 'the filter withheld it' from 'the endpoint refused it' never have a datapoint. Failing check: cloudwatch__sns.filtered-out-and-failed-notification-metrics-count-exactly-the-withheld-and-refused-publishes.

AWS: every accepted Publish increments AWS/SNS NumberOfMessagesPublished under dimension TopicName. For each subscription the message is offered to, a FilterPolicy that does not admit it increments NumberOfNotificationsFilteredOut, and a delivery attempt the endpoint refuses (here an SQS queue whose policy explicitly denies sns.amazonaws.com) increments NumberOfNotificationsFailed. The three are independent series — one does not absorb another — and they are emitted on SNS's own 60 s cadence. Recorded behavior only for the series content: these are documented CloudWatch metric names (https://docs.aws.amazon.com/sns/latest/dg/sns-monitoring-using-cloudwatch.html), not Smithy shapes; the Smithy contract behind the read is cloudwatch 2010-08-01 GetMetricStatistics/GetMetricData and behind the write is sns 2010-03-31 Publish.

Emulator: after 6 publishes (1 admitted by the filter and delivered, 5 withheld by it, all 6 offered to the denying endpoint) the reader never sees a single datapoint for any of the three metrics — the reading is literally None for the whole 420 s budget, i.e. the namespace is empty rather than carrying wrong numbers.

Why: SNS delivery health is invisible. A client cannot distinguish a correctly working filter policy from a silently broken endpoint — the two look identical — cannot alarm on NumberOfNotificationsFailed, and any dashboard or runbook built on the AWS/SNS namespace reads as 'no traffic' for a topic that is actively publishing.

Fix: the SNS publish/fan-out path's metric emission — as each Publish is accepted and each subscription's filter decision and delivery attempt resolves, increment the AWS/SNS NumberOfMessagesPublished, NumberOfNotificationsFilteredOut and NumberOfNotificationsFailed series under {TopicName} in the CloudWatch metric store so GetMetricStatistics/GetMetricData can read them back.

Note: open issue 20261109T183935Z-cloudwatch-sns-2-net-new-causes-the-aws.md describes the same metric family from a different check (cloudwatch__sns.filtered-out-and-failed-notification-metrics-separate-a-filter-from-a-failure) and does not carry this check's replay key; it is named here so the two can be closed together if one fix covers both.

Traces: the failing check id above.

## Evidence
Expected (AWS): AWS/SNS NumberOfMessagesPublished = 6, NumberOfNotificationsFilteredOut = 5 and NumberOfNotificationsFailed = 6 under TopicName=<the topic's name> for 1 admitted + 3 filtered + 2 filtered publishes against one filtered subscription and one denying subscription (the totals are arithmetic over the publishes driven, not fixed literals).
Actual (emulator): `AssertionError: AWS/SNS NumberOfMessagesPublished / NumberOfNotificationsFilteredOut / NumberOfNotificationsFailed for TopicName=<topic name> reaching (6.0, 5.0, 6.0) did not happen within 420.0s; last reading None` — no datapoint of any of the three series ever appears. The topic name and the 420 s budget are per-run; the absent series is the divergence. The positive control in the same check passes, so the delivery pipeline itself is live: only the metric emission is missing.

## Steps to Reproduce
1. CreateTopic. CreateQueue twice: a 'healthy' queue whose policy allows sns.amazonaws.com:SendMessage, and a 'denied' queue whose policy explicitly Denies sns.amazonaws.com:SendMessage.
2. Subscribe both queues. On the healthy subscription SetSubscriptionAttributes FilterPolicy={"probe":["admit"]}; leave the denied subscription with no filter policy.
3. Publish once with MessageAttributes probe=admit (arrives on the healthy queue), then 5 times with probe=withhold.
4. Read AWS/SNS NumberOfMessagesPublished, NumberOfNotificationsFilteredOut and NumberOfNotificationsFailed for TopicName=<topic name> over a 30-minute window with GetMetricStatistics/GetMetricData: no datapoint is ever returned.
