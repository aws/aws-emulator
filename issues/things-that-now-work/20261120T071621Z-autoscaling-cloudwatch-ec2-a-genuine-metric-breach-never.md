Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__cloudwatch__ec2.alarm-state-change-drives-the-scaling-policy-to-launch-an-instance
Issue-Key: autoscaling__cloudwatch__ec2.alarm-state-change-drives-the-scaling-policy-to-launch-an-instance|_poll#22|Timeout|time.sleep(interval_s)
Issue-Commit: 2f24e314505ec85ef2a85cab010fe998301170cd

# autoscaling__cloudwatch__ec2: a genuine metric breach never drives the SimpleScaling policy — the alarm/scale-out scenario makes no observable progress and is cut off at its 600 s budget

> _Note: this issue was identified against the code base at commit `2f24e314505ec85ef2a85cab010fe998301170cd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricAlarm

## Description
Cause 1 of 1: an alarm whose AlarmActions is an Auto Scaling SimpleScaling policy ARN never completes the breach -> ALARM -> policy-invocation -> DesiredCapacity -> new InService instance chain, so the whole scenario is killed at the harness budget (600 s) with no verdict.
AWS: Smithy (cloudwatch PutMetricAlarm) accepts AlarmActions entries that are Auto Scaling scaling-policy ARNs, and (autoscaling PutScalingPolicy) returns a PolicyARN that is exactly such an action target. Recorded behavior for this scenario: after PutMetricData publishes a datapoint above Threshold for the alarm's Period/EvaluationPeriods (Statistic=Maximum, Period=60, EvaluationPeriods=1, DatapointsToAlarm=1, TreatMissingData=notBreaching), (a) DescribeAlarms reports StateValue ALARM, (b) DescribeAlarmHistory/the alarm's action history records an item for that policy ARN with actionState 'Succeeded', (c) the group's DesiredCapacity rises by the policy's ScalingAdjustment, and (d) a scaling activity naming the policy accompanies a second instance reaching InService/running. Every one of those is a server-side effect, and each happens within the check's per-step budgets against AWS.
Emulator: the check reaches none of them inside 600 s — the suite reports only 'Check timed out after 600.0s', so at least one of the steps above never becomes true (the breaching datapoint never evaluating the alarm into ALARM is the first blocking poll, at 600 s with a re-publish every 20 s). No refusal is returned; the state simply never advances, so the scenario yields no verdict at all.
Why: a client that wires CloudWatch alarms to Auto Scaling policies — the standard metric-driven scale-out — observes an alarm that never fires on real data and a group that never grows. Autoscaling behavior cannot be exercised or tested against the emulator at all, and a consumer waiting on DesiredCapacity or on an InService instance waits forever rather than failing.
Fix: the alarm-evaluation loop in the cloudwatch service crate (evaluating published datapoints against Threshold/Period/EvaluationPeriods and transitioning StateValue) together with the alarm-action dispatch to an autoscaling scalingPolicy ARN and the autoscaling side that applies ScalingAdjustment and records the activity. Open issue 20261111T064629Z-autoscaling-cloudwatch-ec2-a-cloudwatch-alarm-whose-alarmaction-is-an-auto.md describes the same hand-off being absent for a SetAlarmState-driven alarm and does not carry this check's replay key; this check additionally requires the alarm to reach ALARM from a REAL PutMetricData breach, which is the step that blocks here.
Traces: autoscaling__cloudwatch__ec2.alarm-state-change-drives-the-scaling-policy-to-launch-an-instance.

## Evidence
autoscaling__cloudwatch__ec2.alarm-state-change-drives-the-scaling-policy-to-launch-an-instance
  expected (AWS): after a breaching PutMetricData, DescribeAlarms -> StateValue 'ALARM'; the alarm's action history has an item for the policy ARN with actionState 'Succeeded'; DescribeAutoScalingGroups -> DesiredCapacity raised by ScalingAdjustment=1; a second instance reaches InService/running with a scaling activity naming the policy.
  actual (emulator): 'Error: Check timed out after 600.0s' — no assertion was ever reached; the first blocking poll (a breaching datapoint re-published every 20 s for up to 600 s, waiting for StateValue ALARM) never satisfied, so neither the action history, nor DesiredCapacity, nor the new instance was observed.
  The alarm itself is accepted and visible: PutMetricAlarm succeeds and DescribeAlarms echoes AlarmActions == [the PutScalingPolicy PolicyARN]; only the evaluation and hand-off never happen.

## Steps to Reproduce
1. Create a launch template and an Auto Scaling group (min 1, max 3, desired 1) and wait for one InService instance.
2. autoscaling PutScalingPolicy(PolicyType='SimpleScaling', AdjustmentType='ChangeInCapacity', ScalingAdjustment=1, Cooldown=0) and keep the returned PolicyARN.
3. cloudwatch PutMetricAlarm(Namespace='sim2real/autoscaling', MetricName='ScaleOutSignal', Dimensions=[{AutoScalingGroupName: <group>}], Statistic='Maximum', Period=60, EvaluationPeriods=1, DatapointsToAlarm=1, Threshold=50, ComparisonOperator='GreaterThanThreshold', TreatMissingData='notBreaching', AlarmActions=[PolicyARN]).
4. PutMetricData for that namespace/metric/dimension with a value of 100, repeating every 20 s, and poll DescribeAlarms for StateValue.
   AWS: StateValue becomes ALARM within a couple of periods, the action history records the policy invocation as Succeeded, DesiredCapacity becomes 2 and a second instance reaches InService.
   Emulator: StateValue never becomes ALARM and nothing downstream moves; the scenario is cut off at 600 s.
