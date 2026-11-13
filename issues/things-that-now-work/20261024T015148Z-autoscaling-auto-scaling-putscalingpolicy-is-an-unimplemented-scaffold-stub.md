Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__cloudformation__cloudwatch__ec2.target-tracking-alb-request-count-without-resource-label-rejected
Issue-Key: autoscaling__cloudformation__cloudwatch__ec2|PutScalingPolicy|NotImplemented
Issue-Commit: 1f316ce023d643c000ad0b8fb7b5799232d706fd

# Auto Scaling PutScalingPolicy is an unimplemented scaffold stub, so no scaling policy can be created

> _Note: this issue was identified against the code base at commit `1f316ce023d643c000ad0b8fb7b5799232d706fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutScalingPolicy

## Description
Cause 1 of 1: PutScalingPolicy does not answer.
AWS: PutScalingPolicy creates or replaces a scaling policy on an existing Auto Scaling group and returns its PolicyARN; for a TargetTrackingScaling policy it also creates the two CloudWatch alarms the policy reports back in DescribePolicies[].Alarms. Recorded behavior only for the alarm pair.
Emulator: NotImplemented — 'operation PutScalingPolicy is scaffolded but not implemented in this emulator'.
Why: the whole Auto Scaling policy surface is unreachable: a client cannot attach any scaling policy to a group, and DescribePolicies/DeletePolicy/ExecutePolicy have nothing to act on. Downstream, the business rule the failing check exists for cannot be observed at all — in AWS a TargetTrackingScaling policy on PredefinedMetricType 'ALBRequestCountPerTarget' with no ResourceLabel is refused with error code ValidationError and the message 'Resource label should be specified for predefined metric type ALBRequestCountPerTarget', leaving no policy and no CloudWatch alarm behind, while the otherwise identical ASGAverageCPUUtilization policy is accepted and gets its two alarms. That rejection is invisible while the operation is a stub, so implementing the handler must include it.
Fix: implement the PutScalingPolicy handler in the autoscaling service (policy state, the target-tracking alarm creation it drives in cloudwatch, and the ResourceLabel validation above).
Traces: autoscaling__cloudformation__cloudwatch__ec2.target-tracking-alb-request-count-without-resource-label-rejected.

## Evidence
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the PutScalingPolicy operation: operation PutScalingPolicy is scaffolded but not implemented in this emulator — raised on the check's POSITIVE control (PolicyType='TargetTrackingScaling', PredefinedMetricType='ASGAverageCPUUtilization', TargetValue=50.0), before the rejection under test was ever reached.

## Steps to Reproduce
Replay autoscaling__cloudformation__cloudwatch__ec2.target-tracking-alb-request-count-without-resource-label-rejected. Minimal repro: create a VPC + subnet + launch template, autoscaling:CreateAutoScalingGroup Min=0/Max=1/Desired=0, then autoscaling:PutScalingPolicy AutoScalingGroupName=<g> PolicyName=<p> PolicyType='TargetTrackingScaling' TargetTrackingConfiguration={'PredefinedMetricSpecification':{'PredefinedMetricType':'ASGAverageCPUUtilization'},'TargetValue':50.0}; then repeat with PredefinedMetricType='ALBRequestCountPerTarget' and no ResourceLabel and expect ValidationError.
