Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__sns.archiving-fifo-topic-reports-archive-processing-metrics cloudwatch__sns.publish-size-metric-reports-the-published-bytes
Issue-Key: cloudwatch__sns.archiving-fifo-topic-reports-archive-processing-metrics|run#10|AssertionError|assert got == 3, f"NumberOfMessagesArchiveProcessing summed to {got!r} for three archived publishes"
Issue-Key: cloudwatch__sns.publish-size-metric-reports-the-published-bytes|run#10|AssertionError|assert points, "PublishSize has no datapoints for a topic whose NumberOfMessagesPublished reached 3"
Issue-Commit: 1a8d6da9d066bb6b81849373d117fc388454286a

# cloudwatch__sns: 2 net-new causes — an archiving FIFO topic publishes no NumberOfMessagesArchiveProcessing, and no topic publishes the PublishSize metric

> _Note: this issue was identified against the code base at commit `1a8d6da9d066bb6b81849373d117fc388454286a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
Two gaps in the AWS/SNS metric set published per topic. In both checks the control metric NumberOfMessagesPublished reaches the driven count (so the publish path and the AWS/SNS pipeline are live), and the divergence is confined to the series below. Both metrics are AWS/SNS vended metrics (recorded behavior only; they are not in the Smithy model) published under the TopicName dimension.

Cause 1 of 2: a FIFO topic with an ArchivePolicy counts nothing in NumberOfMessagesArchiveProcessing.
  AWS: a FIFO topic whose ArchivePolicy sets a MessageRetentionPeriod archives each published message, and the number of messages being archived is published as AWS/SNS NumberOfMessagesArchiveProcessing under the TopicName dimension; three archived publishes sum to 3, beside NumberOfMessagesPublished Sum 3.
  Emulator: CreateTopic with FifoTopic=true and the ArchivePolicy is accepted, the three publishes succeed and NumberOfMessagesPublished reaches Sum 3 (positive control), but NumberOfMessagesArchiveProcessing sums to 0 for the whole budget — the series is never published.
  Why: a client operating a message archive cannot see whether archiving is keeping up; the documented archive-backlog signal reads zero, so replay/retention dashboards and alarms on it never fire.
  Fix: the FIFO publish/archive path must emit NumberOfMessagesArchiveProcessing under (TopicName) for each message it archives.
  Traces: cloudwatch__sns.archiving-fifo-topic-reports-archive-processing-metrics ('NumberOfMessagesArchiveProcessing summed to 0 for three archived publishes').

Cause 2 of 2: PublishSize is never published.
  AWS: every Publish is sampled into the AWS/SNS PublishSize metric under the TopicName dimension, with Unit 'Bytes'; three 1000-byte publishes give PublishSize datapoints whose Maximum is 1000 (the fixed requirement is the metric, its dimension and the Bytes unit, and that the Maximum equals the published message's byte size — the 1000 is this scenario's message size).
  Emulator: NumberOfMessagesPublished reaches Sum 3 (positive control), but GetMetricStatistics for PublishSize returns no datapoints at all for the whole budget.
  Why: a client sizing payloads or watching for messages approaching the topic's MaximumMessageSize has no signal; size-based dashboards and alarms read empty, and the unit contract (Bytes) cannot be relied on.
  Fix: the Publish path's metric emission must sample the message's byte size into PublishSize under (TopicName), with Unit Bytes.
  Traces: cloudwatch__sns.publish-size-metric-reports-the-published-bytes ('PublishSize has no datapoints for a topic whose NumberOfMessagesPublished reached 3').

No open issue carries either replay key: 20261127T143715Z-cloudwatch-sns-sqs-sns-never-publishes-the-numberofnotificationsredriventodlq-metric-for-a.md concerns a different metric and service grouping, and 20261126T150900Z-cloudwatch-sns-the-cfn-deployed-alarm-sns-sqs.md owns a different check of this grouping.

## Evidence
Suite: service cloudwatch__sns scored 15/18 (83%), 3 failed; one is OWNED and skipped.

[mismatch] cloudwatch__sns.archiving-fifo-topic-reports-archive-processing-metrics
  AssertionError: NumberOfMessagesArchiveProcessing summed to 0 for three archived publishes
  Control that passed: NumberOfMessagesPublished (TopicName=<topic>.fifo) reached Sum 3.
  Expected (AWS): GetMetricStatistics(AWS/SNS, NumberOfMessagesArchiveProcessing, [TopicName=<name>], Period 60, Statistics [Sum]) -> Sum 3.
  Actual (emulator): no datapoints (Sum 0).

[mismatch] cloudwatch__sns.publish-size-metric-reports-the-published-bytes
  AssertionError: PublishSize has no datapoints for a topic whose NumberOfMessagesPublished reached 3
  Control that passed: NumberOfMessagesPublished reached Sum 3 for the same topic.
  Expected (AWS): PublishSize datapoints with Unit 'Bytes' and Maximum equal to the published message size (1000 in this scenario).
  Actual (emulator): no datapoints.

Topic names of the recorded runs are per-run values; the requirements are the namespace AWS/SNS, the metric names, the TopicName dimension, the Bytes unit and the counted/measured quantity.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Cause 1: sns:CreateTopic with FifoTopic=true and Attributes {ArchivePolicy: '{"MessageRetentionPeriod":"1"}'}; Publish three messages with MessageGroupId and distinct MessageDeduplicationIds; poll GetMetricStatistics(AWS/SNS, NumberOfMessagesPublished, [TopicName=<name>]) until Sum 3, then GetMetricStatistics(AWS/SNS, NumberOfMessagesArchiveProcessing, [TopicName=<name>]).
   -> AWS: Sum 3. -> Emulator: no datapoints.
3. Cause 2: sns:CreateTopic (standard); Publish three 1000-byte messages; poll NumberOfMessagesPublished until Sum 3, then GetMetricStatistics(AWS/SNS, PublishSize, [TopicName=<name>], Statistics [Maximum]).
   -> AWS: datapoints with Unit 'Bytes' and Maximum 1000. -> Emulator: no datapoints.
4. Re-replay both checks in trace_ids to confirm the fix.
