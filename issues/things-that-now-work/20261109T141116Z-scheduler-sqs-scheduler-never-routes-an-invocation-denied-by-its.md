Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: scheduler__sqs.scheduler-unauthorized-target-role-routes-invocation-to-the-dead-letter-queue
Issue-Key: scheduler__sqs.scheduler-unauthorized-target-role-routes-invocation-to-the-dead-letter-queue|_await_message#20|AssertionError|raise AssertionError(f"{description}: nothing arrived within {timeout_s}s")
Issue-Commit: e2ad54ee7b8ee1b2faa65791f8cb738708e5249b

# scheduler never routes an invocation denied by its Target.RoleArn to the configured dead-letter queue (SQS target)

> _Note: this issue was identified against the code base at commit `e2ad54ee7b8ee1b2faa65791f8cb738708e5249b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSchedule

## Description
This issue carries 1 root cause, in the EventBridge Scheduler occurrence/dispatch component's failed-delivery path.

Cause 1 of 1: an occurrence whose target call is refused because Target.RoleArn is not permitted to make it is never delivered to Target.DeadLetterConfig.Arn.
AWS: Smithy (com.amazonaws.scheduler) — Target.RoleArn is required, @pattern ^arn:aws(-[a-z]+)?:iam::\d{12}:role\/[\w+=,.@\/-]+$, "The Amazon Resource Name (ARN) of the IAM role that EventBridge Scheduler will use for this target when the schedule is invoked." Target.DeadLetterConfig -> DeadLetterConfig, whose Arn is @pattern ^arn:aws(-[a-z]+)?:sqs:[a-z0-9\-]+:\d{12}:[a-zA-Z0-9\-_]+$, documented as the queue to which "EventBridge Scheduler delivers failed events that could not be successfully delivered to a target". An occurrence the assumed role may not deliver is therefore a failed event: the target receives nothing, Target.RetryPolicy is applied, and the event lands on the dead-letter queue.
Emulator: the dead-letter queue stays empty — the check polls it for 420 s and receives nothing — so an unauthorized schedule fails completely silently.
Why: the DLQ is the only observable signal that a scheduled invocation failed; Scheduler reports nothing to the caller at occurrence time and the control plane keeps reporting State=ENABLED. A client that wires DeadLetterConfig to alarm on an unauthorized or broken schedule gets permanent silence, so a schedule that never does its work looks exactly like a healthy one and the failed events are lost.
Fix: the occurrence/dispatch component of the scheduler crate (services/service-com-amazonaws-scheduler-awschronosservice/src/) must classify a target call refused under Target.RoleArn as a delivery failure and, once Target.RetryPolicy is exhausted, SendMessage the failed-event payload to Target.DeadLetterConfig.Arn.
Traces: scheduler__sqs.scheduler-unauthorized-target-role-routes-invocation-to-the-dead-letter-queue.

## Evidence
Replay of simparity-behavioral-aws-v1: scheduler__sqs 2/3 (1 failed).

[mismatch] scheduler__sqs.scheduler-unauthorized-target-role-routes-invocation-to-the-dead-letter-queue
AssertionError: the denied invocation of <schedule name> reaching its configured DLQ: nothing arrived within 420s

Expected: >= 1 message on Target.DeadLetterConfig.Arn describing the failed invocation of the schedule whose Target.RoleArn lacks permission on the target queue.
Actual: 0 messages over a 420 s poll window. The schedule name above is one run's value, not a requirement.

## Steps to Reproduce
1. sqs.create_queue twice: one as the schedule's target, one as the dead-letter queue; read both QueueArns.
2. iam.create_role trusting scheduler.amazonaws.com with an inline policy that allows sqs:SendMessage on the DEAD-LETTER queue only (not on the target queue).
3. scheduler.create_schedule(Name=n, ScheduleExpression='rate(1 minute)', FlexibleTimeWindow={'Mode':'OFF'}, State='ENABLED', Target={'Arn': target_queue_arn, 'RoleArn': role_arn, 'Input': json.dumps({...}), 'RetryPolicy': {'MaximumRetryAttempts': 0}, 'DeadLetterConfig': {'Arn': dlq_arn}}).
4. Poll sqs.receive_message(QueueUrl=dlq_url, WaitTimeSeconds=…) past the first occurrence. On AWS a dead-letter message arrives for the denied invocation. On the emulator nothing arrives (observed: nothing within 420 s).
Check: scheduler__sqs.scheduler-unauthorized-target-role-routes-invocation-to-the-dead-letter-queue.
