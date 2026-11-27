Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__lambda.lifecycle-hook-lambda-target-without-invoke-grant-rejected autoscaling__lambda.lifecycle-hook-lambda-target-without-role-receives-test-notification
Issue-Key: autoscaling__lambda.lifecycle-hook-lambda-target-without-invoke-grant-rejected|_refused#29|AssertionError|raise AssertionError(f"the request was accepted but should have been refused: {answer!r}")
Issue-Key: autoscaling__lambda.lifecycle-hook-lambda-target-without-role-receives-test-notification|run#41|Timeout|time.sleep(2)
Issue-Commit: da5b5c82f86ff32a5f1d8fa609cf9d0b9820cd03

# autoscaling__lambda: 2 net-new causes — a Lambda lifecycle-hook target without an invoke grant is accepted, and a Lambda-target hook is never invoked with the test notification

> _Note: this issue was identified against the code base at commit `da5b5c82f86ff32a5f1d8fa609cf9d0b9820cd03`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutLifecycleHook

## Description
Two net-new root causes on the Lambda lifecycle-hook target path. Neither check is marked ALREADY FILED or OWNED.

Cause 1 of 2: PutLifecycleHook naming a Lambda function that Auto Scaling is not permitted to invoke is accepted.
AWS: PutLifecycleHook publishes a test message to the NotificationTargetARN while handling the call. When the target is a Lambda function whose resource policy does not allow the Auto Scaling service-linked role to invoke it, the publish fails and the call is refused with HTTP 400, error code ValidationError and the message "Unable to publish test message to notification target <function ARN>. Please check your target configuration and try to put lifecycle hook again." — fixed text "Unable to publish test message to notification target " and ". Please check your target configuration and try to put lifecycle hook again.", with the function ARN taken verbatim from the request; DescribeLifecycleHooks lists no hook. After lambda:AddPermission grants the invoke, the identical call is accepted and the hook appears. Smithy (com-amazonaws-autoscaling-autoscaling-2011-01-01): PutLifecycleHook models only LimitExceededFault/ResourceContentionFault, so the refusal and its wording are recorded behavior.
Emulator: the call SUCCEEDS with HTTP 200 and the hook is stored even though nothing may invoke the function.
Why: the client believes its hook is wired when the target can never be driven; on AWS the missing permission is reported at configuration time, here it degrades to notifications that never arrive. A permission-negative test passes here and fails on AWS.
Fix: the PutLifecycleHook handler's target validation / test-publish step — evaluate the Lambda target's invoke permission for the Auto Scaling principal and refuse with the message above.
Traces: autoscaling__lambda.lifecycle-hook-lambda-target-without-invoke-grant-rejected

Cause 2 of 2: a hook with a Lambda NotificationTargetARN and no RoleARN is never invoked with the test notification.
AWS: a lifecycle hook may name a Lambda function as its target with no RoleARN; DescribeLifecycleHooks then lists the hook with that NotificationTargetARN and no RoleARN member, and on creation Auto Scaling invokes the function with an autoscaling:TEST_NOTIFICATION payload naming the group and the caller's AccountId (observable in the function's CloudWatch Logs). Recorded behavior — the test-notification payload is not in any Smithy model.
Emulator: no such invocation is observable; the check makes no progress to a verdict and ends at its replay budget (60 s) while waiting for the function's logged payload.
Why: the Lambda target is the documented serverless way to react to launch/terminate transitions. Without the invocation the client has no signal at all, so hook-driven bootstrap/drain logic hangs rather than failing, and the "is my hook wired" smoke test never completes.
Fix: the PutLifecycleHook/notification-delivery path for Lambda targets — invoke the function with the autoscaling:TEST_NOTIFICATION payload (Service 'AWS Auto Scaling', Event 'autoscaling:TEST_NOTIFICATION', AutoScalingGroupName, AccountId) when the hook is created.
Traces: autoscaling__lambda.lifecycle-hook-lambda-target-without-role-receives-test-notification

## Evidence
Cause 1 — Expected ('ValidationError', 'Unable to publish test message to notification target <function arn>. Please check your target configuration and try to put lifecycle hook again.') for put_lifecycle_hook(NotificationTargetARN=<function arn>, HeartbeatTimeout=60, DefaultResult='CONTINUE') before any lambda:AddPermission.
Actual: AssertionError: the request was accepted but should have been refused: {'ResponseMetadata': {'HTTPStatusCode': 200, ...}} (request id/date are per-run values).

Cause 2 — Expected: describe_lifecycle_hooks lists the hook with NotificationTargetARN == the function ARN and no RoleARN key, and the function's log group holds a payload with Event 'autoscaling:TEST_NOTIFICATION', AutoScalingGroupName == the group and AccountId == the caller's account.
Actual: Check timed out after 60.0s — no invocation payload became observable.

## Steps to Reproduce
Cause 1: create a Lambda function and an Auto Scaling group; without calling lambda:AddPermission for the Auto Scaling service-linked role, call put_lifecycle_hook(LifecycleHookName=<hook>, AutoScalingGroupName=<group>, LifecycleTransition='autoscaling:EC2_INSTANCE_LAUNCHING', NotificationTargetARN=<function arn>, HeartbeatTimeout=60, DefaultResult='CONTINUE'). Observe HTTP 200 and the hook stored instead of the ValidationError above; then add the permission and confirm the identical call is accepted.
Cause 2: create a Lambda function that prints its event, create a group, call put_lifecycle_hook with NotificationTargetARN=<function arn> and no RoleARN, then poll the function's CloudWatch log group for a payload with Event 'autoscaling:TEST_NOTIFICATION'. None arrives.
