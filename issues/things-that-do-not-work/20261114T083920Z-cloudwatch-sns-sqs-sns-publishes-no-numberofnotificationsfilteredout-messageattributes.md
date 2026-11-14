Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__sns__sqs.notifications-filtered-out-and-delivered-are-two-sided-for-complementary-subscriptions
Issue-Key: cloudwatch__sns__sqs.notifications-filtered-out-and-delivered-are-two-sided-for-complementary-subscriptions|run#40|AssertionError|assert 1 <= by_attributes <= filtered, (
Issue-Commit: 48706b3c21ddfe25663178800b6a36ee362c124c

# cloudwatch__sns__sqs: SNS publishes no NumberOfNotificationsFilteredOut-MessageAttributes sub-metric — the per-scope filter series is empty while the aggregate counts 2

> _Note: this issue was identified against the code base at commit `48706b3c21ddfe25663178800b6a36ee362c124c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
Cause 1 of 1: SNS emits the aggregate AWS/SNS `NumberOfNotificationsFilteredOut` counter but none of its filter-scope sub-metrics, so `NumberOfNotificationsFilteredOut-MessageAttributes` has no datapoints at all for a topic whose subscriptions filter on message attributes.

AWS: a subscription whose FilterPolicy rejects a message increments `NumberOfNotificationsFilteredOut` under dimension TopicName, counted per subscription, AND the scope-specific sub-metric for the policy's FilterPolicyScope — `NumberOfNotificationsFilteredOut-MessageAttributes` for the default MessageAttributes scope (the other documented members of the family being `-NoMessageAttributes` and `-InvalidAttributes`). Recorded/documented behavior only: these are CloudWatch metric names from the SNS monitoring documentation (AWS/SNS metric table, TopicName dimension, Sum statistic), not Smithy shapes; the read side is cloudwatch GetMetricStatistics, which correctly returns Datapoints for the aggregate series. AWS does not state that the sub-metrics partition the aggregate, so the requirement is only that the attribute-scope sub-metric carries at least 1 and at most the aggregate count of these rejections.

Emulator: for a fresh topic with two subscriptions carrying complementary message-attribute filter policies and two publishes (each matching exactly one subscription), GetMetricStatistics reports NumberOfMessagesPublished Sum=2, NumberOfNotificationsDelivered Sum=2 and NumberOfNotificationsFilteredOut Sum=2 — all correct — but `NumberOfNotificationsFilteredOut-MessageAttributes` Sum=0, i.e. the series is never published. Assertion: `NumberOfNotificationsFilteredOut-MessageAttributes Sum=0 ..., expected between 1 and the aggregate 2.0`.

Why: a client cannot tell WHY a notification was withheld. The scope sub-metrics are how an operator distinguishes "the attribute filter legitimately rejected it" from "the message carried no attributes at all" or "the attributes were invalid"; a dashboard or alarm built on the per-scope series shows a flat zero line while messages really are being filtered, and a filter-policy misconfiguration is indistinguishable from a healthy filter.

Fix: the SNS publish/delivery path's metric emission — when a subscription's filter policy rejects a message, increment the scope-specific AWS/SNS counter for that subscription's FilterPolicyScope alongside the aggregate NumberOfNotificationsFilteredOut, in the same CloudWatch metric store the aggregate already lands in.

Related open issue, which does NOT cover this: 20261109T183935Z-cloudwatch-sns-2-net-new-causes-the-aws.md (service cloudwatch__sns) says the AWS/SNS notification counters are never published at all. The aggregate counters now ARE published and correct here; what remains missing is only the per-scope sub-metric family, for a different service token and a different check.

Traces: cloudwatch__sns__sqs.notifications-filtered-out-and-delivered-are-two-sided-for-complementary-subscriptions.

## Evidence
Check cloudwatch__sns__sqs.notifications-filtered-out-and-delivered-are-two-sided-for-complementary-subscriptions [mismatch]:
  File "check_cloudwatch__sns__sqs_notifications_filtered_out_and_delivered_are_two_sided_for_complementary_subscriptions.py", line 308, in run
    assert 1 <= by_attributes <= filtered, (
  AssertionError: NumberOfNotificationsFilteredOut-MessageAttributes Sum=0 for <topic name>, expected between 1 and the aggregate 2.0 — both policies scope on message attributes (the default), so the attribute-specific sub-metric must carry these rejections

Expected: GetMetricStatistics(Namespace="AWS/SNS", MetricName="NumberOfNotificationsFilteredOut-MessageAttributes", Dimensions=[{TopicName: <topic>}], Period=60, Statistics=["Sum"]) returns Datapoints summing to >= 1 and <= the aggregate NumberOfNotificationsFilteredOut Sum (2 here: one non-matching subscription per publish, two publishes).
Actual: that call returns no Datapoints (Sum = 0), while the same call for NumberOfMessagesPublished, NumberOfNotificationsDelivered and NumberOfNotificationsFilteredOut returns 2, 2 and 2 respectively and NumberOfNotificationsFailed returns 0. (The topic name and the window are per-run; the empty sub-metric series is the divergence.)

## Steps to Reproduce
1. Create a fresh SNS topic (unique name) and two SQS queues, each with a policy allowing sns.amazonaws.com:SendMessage from that topic.
2. Subscribe both queues with complementary message-attribute filter policies, e.g. {"kind": ["a"]} and {"kind": ["b"]} (default FilterPolicyScope = MessageAttributes).
3. Publish once with MessageAttributes {"kind": {String: "a"}} and once with {"kind": {String: "b"}}; confirm each message reaches exactly one queue.
4. Read AWS/SNS under dimension TopicName with Period=60, Statistics=["Sum"]: NumberOfMessagesPublished, NumberOfNotificationsDelivered, NumberOfNotificationsFilteredOut (all 2), then NumberOfNotificationsFilteredOut-MessageAttributes.
Observed: the sub-metric has no datapoints (Sum=0) instead of 1..2.
