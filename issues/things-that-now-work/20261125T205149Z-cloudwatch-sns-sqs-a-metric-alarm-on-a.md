Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__sns__sqs.deep-cloudwatch-1-cw-warmup-withholds-alarm-and-notification-until-expiry
Issue-Key: cloudwatch__sns__sqs.deep-cloudwatch-1-cw-warmup-withholds-alarm-and-notification-until-expiry|_poll_until#41|AssertionError|raise AssertionError(
Issue-Commit: 77b578d6740dd18bfc5a808294cb3d0be3f0f5dc

# cloudwatch__sns__sqs: a metric alarm on a breaching custom-namespace series never reaches ALARM or invokes its AlarmActions, so no SNS -> SQS notification is ever delivered

> _Note: this issue was identified against the code base at commit `77b578d6740dd18bfc5a808294cb3d0be3f0f5dc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricAlarm

## Description
Cause 1 of 1: A plain metric alarm (no WarmUpConfiguration) over a custom-namespace series that is already breaching never transitions to ALARM and never invokes its AlarmActions, so the alarm -> SNS -> SQS delivery path produces nothing. Failing check: cloudwatch__sns__sqs.deep-cloudwatch-1-cw-warmup-withholds-alarm-and-notification-until-expiry.

AWS: Smithy (com-amazonaws-cloudwatch-graniteserviceversion20100801) PutMetricAlarm takes Namespace/MetricName/Statistic/Period/EvaluationPeriods/DatapointsToAlarm/Threshold/ComparisonOperator plus ActionsEnabled and AlarmActions (ResourceList, @length max 5); WarmUpConfiguration is a separate, optional member, so an alarm created WITHOUT it is subject to no warm-up gating at all. For such an alarm AWS evaluates the configured metric once per Period: when the datapoints in the evaluation window satisfy the comparison (here Statistic=Maximum, Period=60, EvaluationPeriods=1, DatapointsToAlarm=1, ComparisonOperator=GreaterThanThreshold, Threshold=10 against a series published at 100 by PutMetricData, backdated one full period so a complete period is evaluable from the alarm's first evaluation), the alarm transitions INSUFFICIENT_DATA -> ALARM, records a StateUpdate and an Action history item (DescribeAlarmHistory HistoryItemType=Action, HistoryData.actionState=Succeeded naming the target ARN), and performs each AlarmActions entry — for an SNS topic ARN that is an SNS Publish of the alarm-notification document (AlarmName, NewStateValue=ALARM, OldStateValue, StateChangeTime...), which an sqs-protocol subscription then delivers to the queue inside an SNS Notification envelope. The timing (one period plus AWS's evaluation and delivery latency) is recorded behavior only; the required behavior is that the transition and the action happen, not that they happen at a particular second.

Emulator: the configuration half is served correctly — the warm-up arms read WarmUpConfiguration back verbatim, the gated arm is born INSUFFICIENT_DATA with EvaluationState IN_WARM_UP, and the control reports neither member — but the no-warm-up CONTROL alarm never produces anything. Polling the SQS subscriber for 300 s, re-publishing the breaching datum every 5 s, observed zero messages: the harness reports `AssertionError: the no-warm-up control 'sim2real-warmup-control-<id>' delivered its ALARM notification through SNS -> SQS on a series that has been breaching since before it existed did not happen within 300s; last observation {}` — an empty map, i.e. not one alarm notification of any kind arrived on the queue. Hypothesis: either the periodic alarm evaluator never grades PutMetricData datapoints in a custom namespace into a state transition, or the transition happens but the AlarmActions invocation (SNS Publish on the topic ARN) is never performed; the check cannot distinguish them because it is blocked at the SQS barrier before it reads the control's state and Action history.

Why: a client that uses CloudWatch alarms as its alerting path gets silence. Every closed-loop pattern built on the emulator — alarm -> SNS -> SQS/email/HTTPS paging, alarm -> SNS -> Lambda remediation, and any test that asserts an alarm fired by reading DescribeAlarmHistory Action items — can never observe a positive outcome, so a real alerting regression is indistinguishable from a passing run. It also makes every negative-delivery or suppression assertion vacuous: nothing is delivered whether or not it should be, so warm-up gating, DisableAlarmActions and restrictive topic policies cannot be told apart from this defect.

Fix: the CloudWatch metric-alarm evaluator and its action dispatch in the cloudwatch service crate — the periodic evaluation that turns stored metric datapoints into a StateValue transition for a MetricAlarm, and the AlarmActions path that must record an Action history item and issue the SNS Publish for a topic-ARN action. The warm-up configuration path is NOT at fault here: it round-trips correctly and the failing arm has no WarmUpConfiguration.

Traces: cloudwatch__sns__sqs.deep-cloudwatch-1-cw-warmup-withholds-alarm-and-notification-until-expiry (fails at the control barrier, before the warm-up hold and expiry legs are ever graded). Component: services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/src/state.rs and src/ops/ (put_metric_alarm / describe_alarms / describe_alarm_history). Related but NOT covering this check: 20261125T151059Z-cloudwatch-the-disablealarmactions-suppression-scenario-reaches-no-verdict.md (SetAlarmState-driven notification), 20261119T065938Z-cloudwatch-elbv2-s3-a-target-5xx-count-alarm.md (AWS/ApplicationELB series) and 20261124T033148Z-cloudwatch-sns-the-cfn-alarm-restricted-topic-policy.md (CFN-deployed alarm, 60 s cap); this one is a directly PutMetricAlarm-created alarm over a PutMetricData custom namespace with a 300 s budget.

## Evidence
Expected (AWS): after PutMetricAlarm(AlarmName=<control>, Namespace=<custom ns>, MetricName=WarmUpSignal, Statistic=Maximum, Period=60, EvaluationPeriods=1, DatapointsToAlarm=1, Threshold=10.0, ComparisonOperator=GreaterThanThreshold, ActionsEnabled=true, AlarmActions=[<topic arn>]) with the series already at 100 for the current and previous period, DescribeAlarms reports StateValue=ALARM with no EvaluationState member, DescribeAlarmHistory(HistoryItemType=Action) carries an item whose HistoryData.actionState is "Succeeded" and whose HistorySummary names the topic ARN, and the sqs subscription receives an SNS envelope with Type=Notification whose Message is the alarm document with AlarmName=<control>, NewStateValue=ALARM, OldStateValue=INSUFFICIENT_DATA.

Actual (emulator): nothing is delivered. Over 300 s of polling with MaxNumberOfMessages=10/WaitTimeSeconds=2 and a fresh breaching PutMetricData every 5 s, the collected map stays empty:
  AssertionError: the no-warm-up control 'sim2real-warmup-control-81e28199b8c4' delivered its ALARM notification through SNS -> SQS on a series that has been breaching since before it existed did not happen within 300s; last observation {}
(the 81e28199b8c4 suffix and the 300s budget are this run's values, not requirements). The read-back assertions that precede the barrier all pass, so the emulator does model WarmUpConfiguration and EvaluationState=IN_WARM_UP; it is the alarm evaluation / AlarmActions invocation that produces no observable effect.

## Resolution

* Cause 1 of 1 — "a plain metric alarm over an already breaching custom-namespace series never
  delivers its ALARM notification" — is FIXED, at WHEN the transition became due rather than at
  whether it happened. The evaluation and the SNS publish did happen, about half a second after
  `PutMetricAlarm` — inside the sequence of calls the client that wrote the alarms was still
  making. A client writes its alarms, reads them back, clears the action target of whatever was
  waiting there and only THEN begins watching it, so the one publish the transition ever makes had
  already been delivered into the queue and was taken by that clearing read; nothing publishes it
  again, because the alarm now stands in the state it moved to and every later evaluation moves it
  nowhere. The queue therefore stayed empty for the whole barrier.
* The rule applied: an alarm is evaluated on the cadence its own configuration names ("The length,
  in seconds, used each time the metric specified in `MetricName` is evaluated […] the alarm still
  attempts to gather data at the period rate that you specify", `PutMetricAlarmInput$Period`,
  whose shortest regular value is a minute), so the state a written alarm's series implies — and
  the actions the transition into it fires — follow the write by an interval of that cadence and
  never land inside the write's own round trips.
  - `src/state/alarm_evaluation.rs` now carries `WRITE_EVALUATION_DELAY_SECS` (3 s, the span this
    project gives a transition the clock owes) as the emulator's compression of that cadence, beside
    the unchanged re-arm span an evaluation owes the next one.
  - `src/ops/put_metric_alarm.rs` stores the first evaluation of the alarm it writes at that span.
    A composite alarm's rule is evaluated continuously from its children's states and is still due
    at once; a log alarm still runs on its own scheduled-query span; and the evaluation a published
    datapoint makes due is still the re-arm span, since the arrival of data is not the write that
    configured the alarm.
* Pinned end to end by two router suites over the whole mounted service set:
  `core/router/tests/all/cloudwatch_alarm_action_is_owed_a_span_after_the_write.rs` writes the
  alarm over a series that is already breaching, reads it back, CLEARS the subscribed queue and only
  then starts watching — the notification arrives there, `DescribeAlarms` reports `ALARM` and the
  `Action` history item reports `Succeeded` naming the topic, while a warm-up gated arm on the same
  series and topic withholds both; and
  `core/router/tests/all/cloudwatch_breaching_custom_series_alarm_notifies_its_queue.rs` pins the
  other half of the cadence, that the span delays a verdict and never suppresses one.

## Steps to Reproduce
1. sns.CreateTopic; sqs.CreateQueue; read QueueArn via GetQueueAttributes; SetQueueAttributes with a Policy allowing sns.amazonaws.com sqs:SendMessage on the queue for that topic; sns.Subscribe(Protocol="sqs", Endpoint=<queue arn>, ReturnSubscriptionArn=true).
2. cloudwatch.PutMetricData twice into a unique custom namespace for MetricName=WarmUpSignal, Unit=Count, Value=100.0 — once with Timestamp = now-60s and once with Timestamp = now — so a full period is breaching before any alarm exists.
3. cloudwatch.PutMetricAlarm(AlarmName=<control>, Namespace=<that namespace>, MetricName=WarmUpSignal, Statistic=Maximum, Period=60, EvaluationPeriods=1, DatapointsToAlarm=1, Threshold=10.0, ComparisonOperator=GreaterThanThreshold, ActionsEnabled=true, AlarmActions=[<topic arn>]) with NO WarmUpConfiguration.
4. Drain the queue, then poll sqs.ReceiveMessage every 5 s while re-issuing the step-2 PutMetricData for the current minute.
5. Expected within a couple of periods: an SNS Notification envelope whose Message names AlarmName=<control> with NewStateValue=ALARM, and DescribeAlarms/DescribeAlarmHistory showing StateValue=ALARM and a Succeeded Action item naming the topic. Observed: the queue stays empty for the full 300 s and the check fails with `... did not happen within 300s; last observation {}`.
