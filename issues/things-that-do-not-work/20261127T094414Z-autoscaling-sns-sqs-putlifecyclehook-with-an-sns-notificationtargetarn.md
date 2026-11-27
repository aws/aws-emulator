Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__sns__sqs.lifecycle-hook-sns-target-without-role-arn-rejected
Issue-Key: autoscaling__sns__sqs.lifecycle-hook-sns-target-without-role-arn-rejected|_refusal#31|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {result!r}")
Issue-Commit: da5b5c82f86ff32a5f1d8fa609cf9d0b9820cd03

# autoscaling__sns__sqs: PutLifecycleHook with an SNS NotificationTargetARN and no RoleARN is accepted instead of refused with ValidationError

> _Note: this issue was identified against the code base at commit `da5b5c82f86ff32a5f1d8fa609cf9d0b9820cd03`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutLifecycleHook

## Description
Cause 1 of 1: a new lifecycle hook that names an SNS topic as NotificationTargetARN but omits RoleARN is stored instead of refused.
AWS: PutLifecycleHook requires RoleARN whenever NotificationTargetARN is specified. The call is refused with HTTP 400, error code ValidationError and the fixed message "'RoleARN' parameter required when 'NotificationTargetARN' parameter is specified" (no variable parts), and DescribeLifecycleHooks afterwards returns an empty LifecycleHooks list. With the role supplied, the identical call is accepted and the topic receives an autoscaling:TEST_NOTIFICATION. Smithy (com-amazonaws-autoscaling-autoscaling-2011-01-01): PutLifecycleHook's RoleARN and NotificationTargetARN are both optional XmlStringMaxLen255/NotificationTargetResourceName members and the operation models only LimitExceededFault/ResourceContentionFault, so the pairing rule and the message above are recorded behavior.
Emulator: the call SUCCEEDS with HTTP 200 and the hook is stored.
Why: without a role to assume, Auto Scaling has no way to publish to the topic, so the emulator keeps a hook that can never deliver a notification; a client whose IaC or test asserts this refusal passes here and breaks on AWS, and the real-world symptom becomes silently absent notifications rather than a clear configuration error.
Fix: PutLifecycleHook input validation — reject NotificationTargetARN without RoleARN before storing the hook (the same rule is missing on the SQS target path, filed separately for autoscaling__ec2__events__sqs).
Traces: autoscaling__sns__sqs.lifecycle-hook-sns-target-without-role-arn-rejected

## Evidence
Request: put_lifecycle_hook(LifecycleHookName=<hook>, AutoScalingGroupName=<group>, LifecycleTransition='autoscaling:EC2_INSTANCE_LAUNCHING', NotificationTargetARN=<sns topic arn>, HeartbeatTimeout=300, DefaultResult='CONTINUE') — RoleARN omitted.
Expected: ('ValidationError', "'RoleARN' parameter required when 'NotificationTargetARN' parameter is specified") and describe_lifecycle_hooks == [].
Actual: AssertionError: the call was expected to be refused but returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}} (request id and date are per-run values).

## Steps to Reproduce
1. Create an SNS topic (subscribed to an SQS queue) and an Auto Scaling group.
2. Call put_lifecycle_hook with NotificationTargetARN=<topic arn> and no RoleARN.
3. Observe HTTP 200 and a hook listed by describe_lifecycle_hooks instead of HTTP 400 ValidationError "'RoleARN' parameter required when 'NotificationTargetARN' parameter is specified" and an empty hook list.
