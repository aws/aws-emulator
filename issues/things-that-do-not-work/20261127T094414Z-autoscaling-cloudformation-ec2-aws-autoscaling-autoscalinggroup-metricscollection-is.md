Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__cloudformation__ec2.cfn-group-metrics-collection-enables-the-listed-metrics
Issue-Key: autoscaling__cloudformation__ec2.cfn-group-metrics-collection-enables-the-listed-metrics|run#10|AssertionError|assert status == "CREATE_COMPLETE", f"the stack ended {status}: {_stack_failures(ctx, stack)!r}"
Issue-Commit: da5b5c82f86ff32a5f1d8fa609cf9d0b9820cd03

# autoscaling__cloudformation__ec2: AWS::AutoScaling::AutoScalingGroup MetricsCollection is not provisioned — the stack rolls back with an emulator-authored handler reason

> _Note: this issue was identified against the code base at commit `da5b5c82f86ff32a5f1d8fa609cf9d0b9820cd03`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: a CloudFormation stack whose AWS::AutoScaling::AutoScalingGroup declares MetricsCollection cannot be created at all.
AWS: AWS::AutoScaling::AutoScalingGroup supports the MetricsCollection property; CloudFormation passes it to Auto Scaling (the equivalent of EnableMetricsCollection with the listed Metrics and Granularity '1Minute'), the resource reaches CREATE_COMPLETE, the stack reaches CREATE_COMPLETE, and DescribeAutoScalingGroups then reports exactly the listed metrics under EnabledMetrics. Recorded behavior only for this property's provisioning path — the Auto Scaling Smithy model (com-amazonaws-autoscaling-autoscaling-2011-01-01) has EnableMetricsCollection/DescribeAutoScalingGroups.EnabledMetrics, but the CFN resource-to-API mapping is not in any service model.
Emulator: the Group resource fails and the stack ends ROLLBACK_COMPLETE. The failure reason is emulator-authored text that names the emulator itself: 'Resource handler returned message: "this emulator does not send AWS::AutoScaling::AutoScalingGroup''s MetricsCollection property to the service that owns the type, so the resource was not created" (RequestToken: <uuid>, HandlerErrorCode: InvalidRequest)'.
Why: any stack that enables group metrics — a very common pattern for ASG observability templates — cannot be deployed; the client sees a rollback instead of a group, and the reason text is not something AWS ever emits, so it cannot be matched or triaged by a client. A template that deploys on AWS is unusable here.
Fix: the CloudFormation resource provider for AWS::AutoScaling::AutoScalingGroup — forward the MetricsCollection property to the autoscaling EnableMetricsCollection path during create (and update), instead of failing the resource.
Traces: autoscaling__cloudformation__ec2.cfn-group-metrics-collection-enables-the-listed-metrics

## Evidence
Expected: stack status CREATE_COMPLETE and the group's EnabledMetrics equal to the template's MetricsCollection Metrics.
Actual: AssertionError: the stack ended ROLLBACK_COMPLETE: [('Group', 'Resource handler returned message: "this emulator does not send AWS::AutoScaling::AutoScalingGroup\'s MetricsCollection property to the service that owns the type, so the resource was not created" (RequestToken: f0769ed9-27d7-765d-fa9b-9b1f85d281dd, HandlerErrorCode: InvalidRequest)')]
(The RequestToken is a per-run value and is not part of the requirement; the diverging behavior is the resource failure itself and its non-AWS reason text.)

## Steps to Reproduce
Create a stack containing an AWS::AutoScaling::AutoScalingGroup whose properties include MetricsCollection: [{Granularity: '1Minute', Metrics: [GroupInServiceInstances, GroupDesiredCapacity, ...]}], then poll DescribeStacks. Observe ROLLBACK_COMPLETE with the 'Group' resource failure above instead of CREATE_COMPLETE; DescribeAutoScalingGroups never reports the enabled metrics.
