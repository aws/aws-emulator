Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2__events__sqs.launch-hook-sqs-notification-matches-eventbridge-event-and-its-token-completes-the-action autoscaling__ec2__events__sqs.lifecycle-hook-sqs-target-without-role-rejected-and-with-role-notifies-queue-and-eventbridge
Issue-Key: autoscaling__ec2__events__sqs.launch-hook-sqs-notification-matches-eventbridge-event-and-its-token-completes-the-action|_first#27|AssertionError|raise AssertionError(f"no {what} arrived; other messages were {seen!r}")
Issue-Key: autoscaling__ec2__events__sqs.lifecycle-hook-sqs-target-without-role-rejected-and-with-role-notifies-queue-and-eventbridge|_refusal#23|AssertionError|raise AssertionError(f"the call was expected to be refused but succeeded: {got!r}")
Issue-Commit: da5b5c82f86ff32a5f1d8fa609cf9d0b9820cd03

# autoscaling__ec2__events__sqs: 2 net-new causes — a launch lifecycle hook publishes no lifecycle notification on instance launch, and PutLifecycleHook accepts a NotificationTargetARN with no RoleARN

> _Note: this issue was identified against the code base at commit `da5b5c82f86ff32a5f1d8fa609cf9d0b9820cd03`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutLifecycleHook

## Description
Two net-new root causes on the Auto Scaling lifecycle-hook notification path. Neither check is marked ALREADY FILED or OWNED.

Cause 1 of 2: a launching lifecycle hook with an SQS target never receives the lifecycle notification for an actual launch.
AWS: once a hook for LifecycleTransition autoscaling:EC2_INSTANCE_LAUNCHING is registered with a NotificationTargetARN and RoleARN, raising DesiredCapacity holds the new instance in LifecycleState Pending:Wait and publishes a notification to the target carrying LifecycleActionToken (a UUID), AutoScalingGroupName, LifecycleHookName, EC2InstanceId, LifecycleTransition, NotificationMetadata, Origin, Destination, AccountId and RequestId; the same fields appear in the EventBridge "EC2 Instance-launch Lifecycle Action" event from source aws.autoscaling, and the token completes the action so the instance goes InService. Recorded behavior (the notification payload is not in any Smithy model); the fields are the ones the Auto Scaling user guide lists for a lifecycle notification.
Emulator: only the autoscaling:TEST_NOTIFICATION published at PutLifecycleHook time ever arrives on the hook's queue. No message with LifecycleTransition=autoscaling:EC2_INSTANCE_LAUNCHING is published for the launch, so there is no token, no Pending:Wait observation and no EventBridge lifecycle event.
Why: the whole point of a lifecycle hook is the notification and its token — a client that bootstraps or registers an instance during Pending:Wait never gets woken, and CompleteLifecycleAction has no token to use. Scenarios built on hooks stall indefinitely rather than failing.
Fix: the scaling/launch engine's lifecycle-hook notification publication (SQS/SNS target delivery and the matching EventBridge lifecycle event), plus holding the instance in Pending:Wait until the action completes.
Traces: autoscaling__ec2__events__sqs.launch-hook-sqs-notification-matches-eventbridge-event-and-its-token-completes-the-action

Cause 2 of 2: PutLifecycleHook accepts a NotificationTargetARN with no RoleARN.
AWS: a new hook that specifies NotificationTargetARN must also specify RoleARN; otherwise the call is refused with HTTP 400, error code ValidationError and the fixed message "'RoleARN' parameter required when 'NotificationTargetARN' parameter is specified" (no variable parts), and DescribeLifecycleHooks lists no hook. Adding the role to the identical call is accepted. Smithy (com-amazonaws-autoscaling-autoscaling-2011-01-01): PutLifecycleHook models RoleARN and NotificationTargetARN as optional members with only LimitExceeded/ResourceContention errors, so the pairing rule and this message are recorded behavior.
Emulator: the call SUCCEEDS with HTTP 200.
Why: Auto Scaling cannot publish to a target without a role to assume, so the emulator stores a hook that can never deliver; a client's negative test passes here and the same template fails on AWS, and the misconfiguration surfaces only as silently missing notifications.
Fix: PutLifecycleHook input validation (cross-member requirement between NotificationTargetARN and RoleARN) before the hook is stored.
Traces: autoscaling__ec2__events__sqs.lifecycle-hook-sqs-target-without-role-rejected-and-with-role-notifies-queue-and-eventbridge

## Evidence
Cause 1 — after put_lifecycle_hook(LifecycleTransition='autoscaling:EC2_INSTANCE_LAUNCHING', NotificationTargetARN=<queue arn>, RoleARN=<role>, NotificationMetadata=<md>, HeartbeatTimeout=600, DefaultResult='ABANDON') and set_desired_capacity(1):
Expected: a queue message with LifecycleTransition 'autoscaling:EC2_INSTANCE_LAUNCHING', a UUID LifecycleActionToken and the instance in LifecycleState 'Pending:Wait'.
Actual: AssertionError: no lifecycle notification on the hook's queue arrived; other messages were [{'AccountId': ..., 'RequestId': ..., 'AutoScalingGroupARN': ..., 'AutoScalingGroupName': ..., 'Service': 'AWS Auto Scaling', 'Event': 'autoscaling:TEST_NOTIFICATION', 'Time': ...}] — only the test notification.

Cause 2 — Expected ('ValidationError', "'RoleARN' parameter required when 'NotificationTargetARN' parameter is specified") for put_lifecycle_hook(LifecycleHookName='to-queue', NotificationTargetARN=<queue arn>, HeartbeatTimeout=600, DefaultResult='ABANDON') with no RoleARN.
Actual: AssertionError: the call was expected to be refused but succeeded: {'ResponseMetadata': {'HTTPStatusCode': 200, ...}}

## Steps to Reproduce
1. Create a VPC/subnet, launch template and a group at DesiredCapacity 0; create an SQS queue and the IAM role Auto Scaling publishes with, plus an EventBridge rule targeting a second queue.
2. put_lifecycle_hook(... LifecycleTransition='autoscaling:EC2_INSTANCE_LAUNCHING', NotificationTargetARN=<queue arn>, RoleARN=<role arn>, NotificationMetadata=<metadata>, HeartbeatTimeout=600, DefaultResult='ABANDON'), then set_desired_capacity(AutoScalingGroupName=<group>, DesiredCapacity=1). Poll the hook queue: only autoscaling:TEST_NOTIFICATION arrives; no message carries LifecycleTransition/LifecycleActionToken, and describe_auto_scaling_instances never shows Pending:Wait.
3. On a fresh group, call put_lifecycle_hook with NotificationTargetARN set and RoleARN omitted — expect ValidationError, observe HTTP 200 and the hook stored.
