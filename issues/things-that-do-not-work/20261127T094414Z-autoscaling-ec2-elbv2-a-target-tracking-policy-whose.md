Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2__elbv2.target-tracking-resource-label-requires-an-attached-target-group
Issue-Key: autoscaling__ec2__elbv2.target-tracking-resource-label-requires-an-attached-target-group|expect_error#18|AssertionError|raise AssertionError(f"expected the call to be rejected with {(code, message)!r}, but it SUCCEEDED: {result!r}")
Issue-Commit: da5b5c82f86ff32a5f1d8fa609cf9d0b9820cd03

# autoscaling__ec2__elbv2: a target-tracking policy whose ALBRequestCountPerTarget ResourceLabel names an unattached target group is accepted instead of refused

> _Note: this issue was identified against the code base at commit `da5b5c82f86ff32a5f1d8fa609cf9d0b9820cd03`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutScalingPolicy

## Description
Cause 1 of 1: PutScalingPolicy with PredefinedMetricSpecification ALBRequestCountPerTarget and a ResourceLabel naming a target group that is NOT attached to the Auto Scaling group is accepted.
AWS: the ALBRequestCountPerTarget predefined metric is only meaningful for a target group the group is registered with, so Auto Scaling validates the ResourceLabel against the group's attachments and refuses an unattached one with HTTP 400, error code ValidationError and the fixed message "Target group not attached to the Auto Scaling group" (no variable parts). No policy and no alarms are created. Smithy (com-amazonaws-autoscaling-autoscaling-2011-01-01): PutScalingPolicy.TargetTrackingConfiguration carries the PredefinedMetricSpecification with ResourceLabel; the attachment rule and this message are recorded behavior (ValidationError is the awsQuery common sender error, not a modeled shape).
Emulator: the call SUCCEEDS with HTTP 200, returning a PolicyARN and two TargetTracking-<group>-AlarmHigh/AlarmLow alarms for a target group the group is not attached to.
Why: the client is told scaling on per-target request count is configured when there is no path from that target group to the group, so the policy can never scale correctly; the misconfiguration that AWS catches at configuration time is only discovered (or never discovered) at runtime, and a negative test of deployment tooling passes here and fails on AWS.
Fix: PutScalingPolicy validation of TargetTrackingConfiguration.PredefinedMetricSpecification.ResourceLabel against the group's attached target groups / traffic sources.
Traces: autoscaling__ec2__elbv2.target-tracking-resource-label-requires-an-attached-target-group

## Evidence
Expected: ('ValidationError', 'Target group not attached to the Auto Scaling group').
Actual: AssertionError: expected the call to be rejected with ('ValidationError', 'Target group not attached to the Auto Scaling group'), but it SUCCEEDED: {'PolicyARN': 'arn:aws:autoscaling:...:scalingPolicy:.../policyName/requests-per-target', 'Alarms': [{'AlarmName': 'TargetTracking-<group>-AlarmHigh-...'}, {'AlarmName': 'TargetTracking-<group>-AlarmLow-...'}], 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}
(ARNs, alarm ids and the request id in the recording are per-run values; the divergence is the 200 instead of the 400/ValidationError.)

## Steps to Reproduce
1. Create an ALB target group and an Auto Scaling group that is NOT attached to it.
2. put_scaling_policy(AutoScalingGroupName=<group>, PolicyName='requests-per-target', PolicyType='TargetTrackingScaling', TargetTrackingConfiguration={'TargetValue': 100.0, 'PredefinedMetricSpecification': {'PredefinedMetricType': 'ALBRequestCountPerTarget', 'ResourceLabel': '<app/.../targetgroup/...>'}}).
3. Observe HTTP 200 with a PolicyARN and two alarms instead of ValidationError 'Target group not attached to the Auto Scaling group'.
