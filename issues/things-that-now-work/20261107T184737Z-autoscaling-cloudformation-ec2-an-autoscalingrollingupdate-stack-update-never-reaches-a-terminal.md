Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__cloudformation__ec2.cfn-rolling-update-honours-max-batch-size-then-rolls-back-on-a-failed-signal
Issue-Key: autoscaling__cloudformation__ec2.cfn-rolling-update-honours-max-batch-size-then-rolls-back-on-a-failed-signal|_poll_until#9|Timeout|last = probe()
Issue-Commit: 8662809dae52673eae9c053e761f1b93b4d6fc67

# An AutoScalingRollingUpdate stack update never reaches a terminal status, so MaxBatchSize and the failed-signal rollback are unobservable

> _Note: this issue was identified against the code base at commit `8662809dae52673eae9c053e761f1b93b4d6fc67`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack (AWS::AutoScaling::AutoScalingGroup UpdatePolicy AutoScalingRollingUpdate)

## Description
This issue carries 1 net-new root cause.

Cause 1 of 1: an UpdateStack against an Auto Scaling group carrying an AutoScalingRollingUpdate UpdatePolicy never settles — the stack reaches none of the terminal update statuses within the check's 180 s budget, so neither the in-service floor the policy guarantees nor the rollback on an unsignalled batch can be observed.
Failing check ids for this cause: autoscaling__cloudformation__ec2.cfn-rolling-update-honours-max-batch-size-then-rolls-back-on-a-failed-signal.

AWS: recorded behavior only — UpdatePolicy is a template-level directive and is not part of the CloudFormation Smithy model's operation shapes. The recording shows that an update which replaces the group's instances walks them in batches of at most MaxBatchSize while never dropping the group below the stricter of MinInstancesInService and DesiredCapacity - MaxBatchSize, and that when a batch's instances do not signal within WaitOnResourceSignals/PauseTime the stack leaves the update with a terminal status from {UPDATE_COMPLETE, UPDATE_FAILED, UPDATE_ROLLBACK_COMPLETE, UPDATE_ROLLBACK_FAILED, ROLLBACK_COMPLETE, ROLLBACK_FAILED} — a failed signal lands in UPDATE_ROLLBACK_COMPLETE after the prior configuration is restored.
Emulator: the check is killed by the harness and the suite reports exactly 'Error: Check timed out after 180.0s'. No terminal status is ever reached, so no status, no event sequence and no group capacity reading is produced. Hypothesis: the rolling-update driver waits for resource signals it has no path to receive and never times the batch out, so the update neither completes nor fails.
Why: a stack whose ASG uses a rolling UpdatePolicy — the standard way to do a safe instance refresh in CloudFormation — hangs. Callers that poll for a terminal update status block indefinitely, the capacity floor the policy exists to guarantee cannot be checked, and a batch that never signals never rolls back, so a broken deployment is left in flight rather than reverted.
Fix: the CloudFormation provisioning path for AWS::AutoScaling::AutoScalingGroup UpdatePolicy AutoScalingRollingUpdate — batch the replacement by MaxBatchSize while honouring MinInstancesInService, bound each batch by PauseTime when WaitOnResourceSignals is set, and drive the stack to a terminal update status (rolling back on an unsignalled batch) rather than waiting forever.
Traces: autoscaling__cloudformation__ec2.cfn-rolling-update-honours-max-batch-size-then-rolls-back-on-a-failed-signal.

The open issue 20261102T101202Z-autoscaling-cloudformation-ec2-a-replacing-asg-update-with.md carries the replay key of deploy-autoscaling-1-replacing-update-swaps-group-failed-signal-replacement-rejected only; it does not carry this check's key.

## Evidence
Expected (recorded AWS): the update walks the group in batches of at most MaxBatchSize=1, never dropping below DesiredCapacity-MaxBatchSize=2 in service, and the unsignalled batch drives the stack to a terminal status (UPDATE_ROLLBACK_COMPLETE) with the prior launch configuration restored.

Actual (emulator): Error: Check timed out after 180.0s — the stack never reached any of UPDATE_COMPLETE, UPDATE_FAILED, UPDATE_ROLLBACK_COMPLETE, UPDATE_ROLLBACK_FAILED, ROLLBACK_COMPLETE, ROLLBACK_FAILED.

## Steps to Reproduce
1. CreateStack with an AWS::AutoScaling::AutoScalingGroup at DesiredCapacity 3 whose UpdatePolicy sets AutoScalingRollingUpdate with MaxBatchSize 1, MinInstancesInService 1 and WaitOnResourceSignals true.
2. UpdateStack with a change that forces instance replacement, where the new instances never send a resource signal.
3. Poll DescribeStacks for a terminal update status.
4. Observe that no terminal status is ever reached.
Or replay the check id in trace_ids.
