Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ecs__sns__sqs.obs-ecs-1-alarm-notification-withheld-from-queue-without-sns-policy
Issue-Key: cloudwatch__ecs__sns__sqs.obs-ecs-1-alarm-notification-withheld-from-queue-without-sns-policy|_poll_until#26|Timeout|time.sleep(delay)
Issue-Commit: 2c1da22bfb6e6388a411330b9028a26373fd79c1

# cloudwatch__ecs__sns__sqs: the ECS Container-Insights alarm -> SNS -> SQS fan-out still produces no observable step — the withheld-notification check is killed at its cap with no verdict (1 cause)

> _Note: this issue was identified against the code base at commit `2c1da22bfb6e6388a411330b9028a26373fd79c1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricAlarm

## Description
One net-new root cause, grounded in the only failing check under the suite heading `### cloudwatch__ecs__sns__sqs` that the replay report marks neither ALREADY FILED nor OWNED.

NOTE ON RELATED ISSUES: the three issues on this group are all in the RESOLVED folder — 20261025T020208Z-cloudwatch-ecs-sns-sqs-an-ecs-runningtaskcount-alarm-never-fires-so-sns.md, 20261025T043441Z-cloudwatch-ecs-sns-sqs-sns-sqs-notification-delivery-ignores-the-destination-queue.md and 20261117T145304Z-cloudwatch-ecs-sns-sqs-the-ecs-container-insights.md (whose key for this check is `_poll_until#26|Timeout|time.sleep(delay)`). A resolved issue never covers a failure, and the check still reaches no observable step, so the divergence is filed here.

Cause 1 of 1: an alarm driven by an ECS Container-Insights metric never produces the ALARM -> SNS -> SQS hand-off the scenario is built on, so no assertion in the check is ever reached.
Failing check id: cloudwatch__ecs__sns__sqs.obs-ecs-1-alarm-notification-withheld-from-queue-without-sns-policy

AWS: Smithy (cloudwatch) PutMetricAlarm accepts Namespace/MetricName/Statistic/Period/EvaluationPeriods/Threshold/ComparisonOperator/Dimensions/TreatMissingData plus AlarmActions (ResourceList, @length max 5); DescribeAlarms -> MetricAlarm.StateValue targets com.amazonaws.cloudwatch#StateValue with members OK, ALARM, INSUFFICIENT_DATA, and SetAlarmState drives an explicit transition. On AWS, a Container-Insights-enabled cluster whose service has desiredCount 0 reports ECS/ContainerInsights RunningTaskCount below 1 (or no datapoint, which TreatMissingData=breaching treats as breaching), so the alarm settles in ALARM on its own; each OK->ALARM transition publishes exactly one notification to every topic in AlarmActions, and SNS delivers that notification to each subscribed SQS queue whose queue policy grants sqs:SendMessage to the topic and delivers nothing to a subscribed queue with no such grant — adding the grant makes the previously silent queue receive the next round. The notification body carries AlarmName, NewStateValue=ALARM, the NewStateReason given to SetAlarmState, and Trigger.MetricName=RunningTaskCount. Recorded behavior only for the evaluation and the SNS->SQS authorization decision (no model text covers either).
Emulator: the check's entire detail is the single line `Error: Check timed out after 60.0s`. It never reaches the delivery assertion on the granted queue, the absence assertion on the ungranted queue, or the post-repair assertion. The check's internal budgets are all larger than the cap it died at (420 s waiting for StateValue=ALARM, 180 s per receive round, 300 s for the repaired queue), so no assertion fired: no step of the chain produced its effect. Hypothesis: either ECS/ContainerInsights RunningTaskCount is never published for a zero-task service so the alarm never leaves INSUFFICIENT_DATA, or the ALARM transition (including the explicit SetAlarmState rounds) never hands off to the AlarmActions topic.
Why: alarm -> SNS -> SQS is the standard CloudWatch notification fan-out. A client that provisions an alarm with an SNS action observes silence for a condition AWS alarms on, and the negative half of the contract is untestable because nothing is delivered to any queue — a missing sqs:SendMessage grant and a dead pipeline become indistinguishable.
Fix: the CloudWatch alarm evaluator and its action hand-off in the cloudwatch crate (the component that moves a metric alarm to ALARM — on evaluation and on SetAlarmState — and publishes one notification per transition to each AlarmActions topic), plus the ECS Container-Insights RunningTaskCount publication that feeds the metric store for a zero-task service.
Traces: cloudwatch__ecs__sns__sqs.obs-ecs-1-alarm-notification-withheld-from-queue-without-sns-policy

## Evidence
Failing check (suite heading `### cloudwatch__ecs__sns__sqs`): obs-ecs-1-alarm-notification-withheld-from-queue-without-sns-policy — verdict [mismatch], detail in full: `Error: Check timed out after 60.0s` — no assertion reached.

Setup the check completes before the stall: ecs:CreateCluster with containerInsights=enabled; ecs:RegisterTaskDefinition (EC2/bridge, one container); ecs:CreateService with desiredCount 0; sns:CreateTopic; two sqs:CreateQueue; sns:Subscribe both queues with Protocol=sqs; a Policy on only ONE queue allowing sqs:SendMessage for Principal Service sns.amazonaws.com with Condition ArnEquals aws:SourceArn = the topic ARN (the check asserts the other queue has no Policy attribute at all, and that assertion passes); cloudwatch:PutMetricAlarm on ECS/ContainerInsights RunningTaskCount, Statistic=Maximum, Period=60, EvaluationPeriods=1, Threshold=1.0, ComparisonOperator=LessThanThreshold, TreatMissingData=breaching, AlarmActions=[topic].

Expected (AWS): DescribeAlarms reports StateValue=ALARM without intervention; each SetAlarmState OK->ALARM round delivers one notification to the granted queue whose payload has AlarmName=<the alarm>, NewStateValue=ALARM, a NewStateReason ending in the round token and Trigger.MetricName=RunningTaskCount; the ungranted queue stays empty (ApproximateNumberOfMessages + ApproximateNumberOfMessagesNotVisible = 0) across all 3 rounds; after the grant is added that same queue receives the next round.

Actual (emulator): none of those observations occurs inside the budget — the alarm/notification chain produced no observable effect and the check was killed with no verdict.

## Steps to Reproduce
1. ecs:CreateCluster(settings=[{name: containerInsights, value: enabled}]); ecs:RegisterTaskDefinition (EC2/bridge, one container); ecs:CreateService in that cluster with desiredCount=0.
2. sns:CreateTopic; sqs:CreateQueue twice; sns:Subscribe both queue ARNs with Protocol=sqs; sqs:SetQueueAttributes on ONE queue with a Policy allowing sqs:SendMessage to Principal Service sns.amazonaws.com conditioned on aws:SourceArn = the topic ARN; leave the other queue with no Policy.
3. cloudwatch:PutMetricAlarm on ECS/ContainerInsights RunningTaskCount for that ClusterName/ServiceName with Statistic=Maximum, Period=60, EvaluationPeriods=1, Threshold=1, ComparisonOperator=LessThanThreshold, TreatMissingData=breaching, AlarmActions=[topic ARN].
4. Poll cloudwatch:DescribeAlarms(AlarmNames=[alarm], AlarmTypes=[MetricAlarm]) for StateValue=ALARM; then for each round call SetAlarmState OK then ALARM with a tagged StateReason and sqs:ReceiveMessage on both queues. EXPECT ALARM, one tagged notification per round on the granted queue, nothing on the ungranted one; then grant the ungranted queue and EXPECT the next round to arrive there with Trigger.MetricName=RunningTaskCount. OBSERVE no alarm transition and no delivery within the budget.
5. Re-replay cloudwatch__ecs__sns__sqs.obs-ecs-1-alarm-notification-withheld-from-queue-without-sns-policy.
