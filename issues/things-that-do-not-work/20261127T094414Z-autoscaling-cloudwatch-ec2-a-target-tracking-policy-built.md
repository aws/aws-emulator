Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__cloudwatch__ec2.target-tracking-metric-math-policy-creates-its-alarms
Issue-Key: autoscaling__cloudwatch__ec2.target-tracking-metric-math-policy-creates-its-alarms|run#12|AssertionError|assert len(names) == 2, f"the target tracking policy returned alarms {resp['Alarms']!r}"
Issue-Commit: da5b5c82f86ff32a5f1d8fa609cf9d0b9820cd03

# autoscaling__cloudwatch__ec2: a target-tracking policy built on a metric-math CustomizedMetricSpecification creates no CloudWatch alarms (PutScalingPolicy returns an empty Alarms list)

> _Note: this issue was identified against the code base at commit `da5b5c82f86ff32a5f1d8fa609cf9d0b9820cd03`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutScalingPolicy

## Description
Cause 1 of 1: PutScalingPolicy with PolicyType TargetTrackingScaling and a TargetTrackingConfiguration whose CustomizedMetricSpecification uses the Metrics (metric-math) form returns no alarms and creates none in CloudWatch.
AWS: every target-tracking policy — predefined metric, customized single metric, or metric-math Metrics list — is provisioned with two CloudWatch alarms (an AlarmHigh and an AlarmLow named TargetTracking-<group>-AlarmHigh-<id> / -AlarmLow-<id>). PutScalingPolicy returns them in its output: Smithy (com-amazonaws-autoscaling-autoscaling-2011-01-01) PutScalingPolicy output = PolicyARNType {PolicyARN, Alarms: Alarms}. DescribeAlarms then lists both, and each alarm's Metrics carries the policy's expression on the ReturnData query (here "m1 / 2"). Recorded behavior for the alarm count/naming; the Alarms output member is modeled.
Emulator: the same call succeeds but returns Alarms: [] — no alarm is created, so DescribeAlarms has nothing to list and the expression is never evaluated anywhere. The emulator DOES return two alarms for a predefined-metric target-tracking policy (observed on the elbv2 path), so only the metric-math/customized-Metrics branch is unwired.
Why: the alarms are the only thing that drives a target-tracking policy. A client that configures backlog-per-instance style scaling (the canonical metric-math use case) gets a policy that can never fire, and a client that reads resp['Alarms'] to tag, describe or clean up its alarms gets an empty list and silently leaks/misses them.
Fix: the PutScalingPolicy handler's target-tracking alarm provisioning — handle CustomizedMetricSpecification.Metrics (metric math) alongside PredefinedMetricSpecification, and serialize the created alarms into the Alarms output member.
Traces: autoscaling__cloudwatch__ec2.target-tracking-metric-math-policy-creates-its-alarms

## Evidence
Request: put_scaling_policy(PolicyType='TargetTrackingScaling', TargetTrackingConfiguration={'TargetValue': 10.0, 'CustomizedMetricSpecification': {'Metrics': [{'Id': 'm1', 'ReturnData': False, 'MetricStat': {...Sum...}}, {'Id': 'e1', 'Expression': 'm1 / 2', 'ReturnData': True, 'Label': 'per-instance'}]}}).
Expected: Alarms with 2 entries (AlarmHigh + AlarmLow), both listed by DescribeAlarms, each evaluating ['m1 / 2'] on its ReturnData query.
Actual: AssertionError: the target tracking policy returned alarms []
For contrast, a predefined-metric target-tracking policy on the same emulator returns two TargetTracking-<group>-AlarmHigh/AlarmLow entries, so the divergence is specific to the metric-math form.

## Steps to Reproduce
1. Create a group from a launch template across two subnets.
2. put_scaling_policy(AutoScalingGroupName=<group>, PolicyName='backlog-per-instance', PolicyType='TargetTrackingScaling', TargetTrackingConfiguration={'TargetValue': 10.0, 'CustomizedMetricSpecification': {'Metrics': [{'Id':'m1','ReturnData':False,'MetricStat':{'Metric':{'Namespace':'<ns>','MetricName':'Backlog'},'Stat':'Sum'}}, {'Id':'e1','Expression':'m1 / 2','ReturnData':True,'Label':'per-instance'}]}}).
3. Inspect the response's Alarms (empty) and call cloudwatch DescribeAlarms for TargetTracking-<group>-* (nothing).
