Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__sqs.lifecycle-hook-fifo-queue-target-rejected
Issue-Key: autoscaling__sqs.lifecycle-hook-fifo-queue-target-rejected|_refusal#16|AssertionError|raise AssertionError(f"the call was expected to be refused but succeeded: {got!r}")
Issue-Commit: da5b5c82f86ff32a5f1d8fa609cf9d0b9820cd03

# autoscaling__sqs: PutLifecycleHook accepts a FIFO queue as the notification target instead of refusing the undeliverable target

> _Note: this issue was identified against the code base at commit `da5b5c82f86ff32a5f1d8fa609cf9d0b9820cd03`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutLifecycleHook

## Description
Cause 1 of 1: a lifecycle hook whose NotificationTargetARN is a FIFO (.fifo) SQS queue is accepted.
AWS: Auto Scaling does not support FIFO queues as lifecycle notification targets. PutLifecycleHook publishes a test message to the target with the supplied role while the call is being handled, and when that publish fails the call is refused with HTTP 400, error code ValidationError and the message "Unable to publish test message to notification target <queue ARN> using IAM role <role ARN>. Please check your target and role configuration and try to put lifecycle hook again." — fixed text "Unable to publish test message to notification target " / " using IAM role " / ". Please check your target and role configuration and try to put lifecycle hook again.", with the two ARNs taken verbatim from the request. A standard queue with the same role is accepted and receives autoscaling:TEST_NOTIFICATION. Smithy (com-amazonaws-autoscaling-autoscaling-2011-01-01): PutLifecycleHook models only LimitExceededFault/ResourceContentionFault, so the refusal and its wording are recorded behavior.
Emulator: the FIFO-target call SUCCEEDS with HTTP 200 (the positive standard-queue control in the same check passes, so only the undeliverable-target refusal is missing).
Why: the client is told the hook is configured; on AWS the same request fails fast and the operator fixes the target, whereas here the hook exists and its notifications are simply never usable. A deployment that is invalid on AWS is reported as valid.
Fix: the PutLifecycleHook handler's target validation / test-publish step — attempt (or model) the test publish to the NotificationTargetARN with the given RoleARN and refuse with the message above when the target cannot receive it.
Traces: autoscaling__sqs.lifecycle-hook-fifo-queue-target-rejected

## Evidence
Request: put_lifecycle_hook(LifecycleHookName='to-fifo', AutoScalingGroupName=<group>, LifecycleTransition='autoscaling:EC2_INSTANCE_LAUNCHING', NotificationTargetARN=<...-queue.fifo arn>, RoleARN=<role arn>).
Expected (with the ARNs masked by the check): ('ValidationError', 'Unable to publish test message to notification target <queue-arn> using IAM role <role-arn>. Please check your target and role configuration and try to put lifecycle hook again.'), the refusal naming both the FIFO queue and the role.
Actual: AssertionError: the call was expected to be refused but succeeded: {'ResponseMetadata': {'HTTPStatusCode': 200, ...}}
In the same check the standard-queue hook was accepted and received exactly one autoscaling:TEST_NOTIFICATION, so the emulator's divergence is limited to not refusing the FIFO target.

## Steps to Reproduce
1. Create a standard SQS queue and a .fifo SQS queue, plus the IAM role Auto Scaling uses to publish, and an Auto Scaling group.
2. put_lifecycle_hook(... NotificationTargetARN=<standard queue arn>, RoleARN=<role>) — accepted, one autoscaling:TEST_NOTIFICATION arrives (control).
3. put_lifecycle_hook(LifecycleHookName='to-fifo', ... NotificationTargetARN=<fifo queue arn>, RoleARN=<role>) — observe HTTP 200 instead of HTTP 400 ValidationError 'Unable to publish test message to notification target <queue arn> using IAM role <role arn>. Please check your target and role configuration and try to put lifecycle hook again.'
