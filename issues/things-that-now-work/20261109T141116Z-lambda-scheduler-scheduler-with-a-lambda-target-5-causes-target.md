Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__scheduler.cfn-deploy-scheduler-missing-invoke-permission-negative lambda__scheduler.scheduler-lambda-missing-invoke-permission-negative lambda__scheduler.scheduler-lambda-flexible-time-window-invocation lambda__scheduler.scheduler-lambda-one-time-schedule-invocation-positive lambda__scheduler.cli-scheduler-update-schedule-changes-target lambda__scheduler.scheduler-lambda-retry-policy-invocation-count
Issue-Key: lambda__scheduler.cfn-deploy-scheduler-missing-invoke-permission-negative|run#17|AssertionError|assert broken_messages == [], (
Issue-Key: lambda__scheduler.scheduler-lambda-missing-invoke-permission-negative|run#17|AssertionError|assert len(bad_msgs) == 0, f"schedule invoked Lambda despite missing invoke permission: {bad_msgs}"
Issue-Key: lambda__scheduler.scheduler-lambda-flexible-time-window-invocation|run#19|AssertionError|assert sent_ms >= nominal_ms - slack_ms, (
Issue-Key: lambda__scheduler.scheduler-lambda-one-time-schedule-invocation-positive|_poll_until#16|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: lambda__scheduler.cli-scheduler-update-schedule-changes-target|run#23|AssertionError|assert not a_events_after, (
Issue-Key: lambda__scheduler.scheduler-lambda-retry-policy-invocation-count|run#14|AssertionError|assert len(collected) == 1, (
Issue-Commit: e2ad54ee7b8ee1b2faa65791f8cb738708e5249b

# scheduler with a Lambda target: 5 causes — Target.RoleArn permission never enforced, occurrence fires ~1 min early, ActionAfterCompletion never deletes, UpdateSchedule keeps the old target armed, and RetryPolicy retries a function-level failure

> _Note: this issue was identified against the code base at commit `e2ad54ee7b8ee1b2faa65791f8cb738708e5249b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSchedule

## Description
This issue carries 5 root causes, all in the EventBridge Scheduler occurrence/dispatch component as exercised against Lambda targets. Each must be closed before this issue is submitted; each cause lists the failing check ids it is grounded in. None is marked ALREADY FILED or OWNED in the replay report.

=== Cause 1 of 5: Target.RoleArn is never authorized at invocation time, so a schedule whose role lacks lambda:InvokeFunction invokes the function anyway ===
Failing checks: lambda__scheduler.cfn-deploy-scheduler-missing-invoke-permission-negative, lambda__scheduler.scheduler-lambda-missing-invoke-permission-negative
AWS: Smithy (com.amazonaws.scheduler) — Target.RoleArn is required, @pattern ^arn:aws(-[a-z]+)?:iam::\d{12}:role\/[\w+=,.@\/-]+$, documented as "The Amazon Resource Name (ARN) of the IAM role that EventBridge Scheduler will use for this target when the schedule is invoked." Every occurrence is delivered AS that role, so when the role's policy does not allow lambda:InvokeFunction on the target function the invocation is refused: the function never runs, and the occurrence is a failed event (retried per RetryPolicy, then sent to DeadLetterConfig.Arn if one is configured).
Emulator: the function IS invoked, repeatedly. The check observes a list of actual invocations with their request ids where AWS produces none at all.
Why: the role is the only authorization boundary at invocation time. A client that relies on a narrowly scoped execution role to keep a schedule from touching a function gets the invocation anyway — a least-privilege control that exists in AWS is absent, so a misconfigured or malicious schedule executes work it has no permission for, and negative tests that assert "no invocation without permission" pass against AWS and fail here.
Fix: the occurrence/dispatch component of the scheduler crate (services/service-com-amazonaws-scheduler-awschronosservice/src/) must evaluate the target call under Target.RoleArn before issuing it and treat an authorization refusal as a delivery failure rather than invoking unconditionally.
Traces: lambda__scheduler.cfn-deploy-scheduler-missing-invoke-permission-negative (schedule created by a CloudFormation stack), lambda__scheduler.scheduler-lambda-missing-invoke-permission-negative (schedule created through the Scheduler API).

=== Cause 2 of 5: an occurrence is delivered BEFORE its nominal instant ===
Failing check: lambda__scheduler.scheduler-lambda-flexible-time-window-invocation
AWS: Smithy (com.amazonaws.scheduler) — FlexibleTimeWindow (required on CreateSchedule) has Mode -> FlexibleTimeWindowMode (required) "Determines whether the schedule is invoked within a flexible time window" and MaximumWindowInMinutes -> integer @range 1..1440 "The maximum time window during which a schedule can be invoked." The nominal instant comes from ScheduleExpression (+ ScheduleExpressionTimezone). With Mode=FLEXIBLE the actual invocation instant is chosen by the service inside [nominal, nominal + MaximumWindowInMinutes]; with Mode=OFF it is the nominal instant itself. In NEITHER mode may an occurrence be delivered before the nominal instant.
Emulator: the Lambda target receives the invocation ~57 s BEFORE the nominal instant — the window is applied in the wrong direction (or the nominal instant is not used as the lower bound at all).
Why: firing early is the one error a time-based service must not make: a client that uses the flexible window to spread load, or that depends on a schedule not running before its nominal time (data not yet present, a window not yet open), gets work executed against state that does not exist yet. The bound is also silent — the schedule reads back exactly as configured.
Fix: the occurrence computation in the dispatch component must clamp the chosen invocation instant at BOTH ends of [nominal, nominal + MaximumWindowInMinutes] and never schedule before the nominal instant.
Traces: lambda__scheduler.scheduler-lambda-flexible-time-window-invocation.

=== Cause 3 of 5: ActionAfterCompletion=DELETE never removes a one-time schedule after its single invocation ===
Failing check: lambda__scheduler.scheduler-lambda-one-time-schedule-invocation-positive
AWS: Smithy (com.amazonaws.scheduler) — ActionAfterCompletion is an enum with values NONE | DELETE; CreateScheduleInput.ActionAfterCompletion is documented as "Specifies the action that EventBridge Scheduler applies to the schedule after the schedule completes invoking the target." So a one-time at() schedule with ActionAfterCompletion=DELETE is removed by the service once it has completed invoking its target, after which GetSchedule answers ResourceNotFoundException (404) with 'Schedule <Name> does not exist.'
Emulator: the schedule is invoked, but it still resolves afterwards — the auto-delete condition is never met within the check's 120 s budget (last result: False).
Why: ActionAfterCompletion=DELETE is how clients keep one-shot schedules from accumulating. A client gets a store that grows without bound and a reconciler that keeps seeing a completed schedule as live; a test that waits for the schedule to disappear hangs.
Fix: the dispatch component must apply the record's ActionAfterCompletion when a schedule completes (a one-time expression that has fired, or a recurring one that has passed its EndDate), deleting the record so GetSchedule 404s.
Traces: lambda__scheduler.scheduler-lambda-one-time-schedule-invocation-positive.

=== Cause 4 of 5: UpdateSchedule does not re-arm the occurrence against the NEW Target — the previous target keeps being invoked ===
Failing check: lambda__scheduler.cli-scheduler-update-schedule-changes-target
AWS: UpdateSchedule replaces the schedule definition wholesale (Target is a required member of the update, with Arn and RoleArn required inside it). After it returns 200, every subsequent occurrence is delivered to the NEW Target.Arn and the previous target receives nothing more; GetSchedule reports the new target. Recorded behavior: no invocation of the old function is observed after the UpdateSchedule response.
Emulator: function A — the target the schedule was switched AWAY from — receives an invocation after the UpdateSchedule call returned.
Why: a client that re-points a schedule (blue/green cutover, moving work to a new function) gets the old target still running, so work is executed twice or against a decommissioned component, while the control plane reports only the new target. There is nothing the client can observe to detect it.
Fix: UpdateSchedule (services/service-com-amazonaws-scheduler-awschronosservice/src/ops/update_schedule.rs) must disarm the schedule's existing armed occurrence and re-arm it from the updated record before responding, so no occurrence dispatched after the response can carry the previous Target.
Traces: lambda__scheduler.cli-scheduler-update-schedule-changes-target.

=== Cause 5 of 5: RetryPolicy is applied to a Lambda target's own FUNCTION-level failure, producing extra invocations ===
Failing check: lambda__scheduler.scheduler-lambda-retry-policy-invocation-count
AWS: Smithy (com.amazonaws.scheduler) — RetryPolicy.MaximumRetryAttempts is an integer @range 0..185, "The maximum number of retry attempts to make before the request fails", and RetryPolicy is documented as the policy for "the maximum number of times EventBridge Scheduler will try to DELIVER the event to a target". A templated Lambda invocation that is DELIVERED successfully and whose handler then raises is not a delivery failure: the Invoke call succeeded (HTTP 200 with FunctionError), so Scheduler does not retry it. One occurrence therefore produces exactly one invocation regardless of MaximumRetryAttempts.
Emulator: the same occurrence produces 3 invocations with identical input — the retry policy is applied to the function's own failure.
Why: the client's handler is executed N+1 times for one occurrence. For any non-idempotent handler that is duplicated work — duplicate writes, duplicate notifications, duplicate charges — and the duplication scales with a MaximumRetryAttempts the client set expecting it to govern delivery only.
Fix: the dispatch component must distinguish a failed DELIVERY (the target call itself was refused or errored at the transport/authorization level) from a successful delivery whose target reported a function-level error, and apply RetryPolicy only to the former.
Traces: lambda__scheduler.scheduler-lambda-retry-policy-invocation-count.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator: 33/171 checks passed overall; lambda__scheduler 15/29 (6 failed, 8 unloadable). The six failures below are neither ALREADY FILED nor OWNED in the report. Concrete names, request ids and timestamps are one run's values, not requirements.

Cause 1 — Target.RoleArn not authorized at invocation time:
  [mismatch] lambda__scheduler.cfn-deploy-scheduler-missing-invoke-permission-negative — AssertionError: schedule whose role lacks lambda:InvokeFunction must never invoke the target, got [{'invoked': True, 'requestId': ...}, ... ] (dozens of invocations observed).
  [mismatch] lambda__scheduler.scheduler-lambda-missing-invoke-permission-negative — AssertionError: schedule invoked Lambda despite missing invoke permission: [{'event': {'marker': 'bad'}} x5].
  Expected: zero invocations; the occurrence fails and (when configured) dead-letters.

Cause 2 — early fire:
  [mismatch] lambda__scheduler.scheduler-lambda-flexible-time-window-invocation — AssertionError: flexible-window invocation reached the Lambda target before its nominal fire time: sent=1794232720177 nominal=1794232777000 (≈56.8 s early).
  Expected: invocation instant inside [nominal, nominal + MaximumWindowInMinutes]; never before nominal.

Cause 3 — ActionAfterCompletion=DELETE:
  [mismatch] lambda__scheduler.scheduler-lambda-one-time-schedule-invocation-positive — AssertionError: schedule auto-deleted after its single invocation completed not met within 120.0s. Last result: False.
  Expected: after the single invocation, GetSchedule -> 404 ResourceNotFoundException 'Schedule <Name> does not exist.'

Cause 4 — UpdateSchedule does not re-arm:
  [mismatch] lambda__scheduler.cli-scheduler-update-schedule-changes-target — AssertionError: function A received 1 invocation(s) after the target was switched away from it at 1794228568275; last seen at 1794228573583.
  Expected: no invocation of the previous target after UpdateSchedule returns.

Cause 5 — RetryPolicy on a function-level failure:
  [mismatch] lambda__scheduler.scheduler-lambda-retry-policy-invocation-count — AssertionError: expected exactly 1 invocation (RetryPolicy must not retry a Lambda target's own function-level failure), observed 3: [{'event': {'marker': 'retry-test'}} x3].
  Expected: exactly one invocation per occurrence.

## Steps to Reproduce
Common setup: lambda.create_function for a function that records each invocation (and, for Cause 5, raises); sqs.create_queue where a DLQ is needed; iam.create_role trusting scheduler.amazonaws.com. All Scheduler calls via boto3 client('scheduler') against the emulator.

Cause 1: create the role WITHOUT lambda:InvokeFunction on the function. scheduler.create_schedule(Name=n, ScheduleExpression='rate(1 minute)', FlexibleTimeWindow={'Mode':'OFF'}, State='ENABLED', Target={'Arn': function_arn, 'RoleArn': role_arn, 'Input': json.dumps({'marker':'bad'})}). Read the function's invocation record for several minutes: on AWS it stays empty; on the emulator invocations accumulate. Repeat with the same schedule created by a CloudFormation stack (AWS::Scheduler::Schedule + AWS::IAM::Role without the invoke permission) for the CFN variant. Checks lambda__scheduler.scheduler-lambda-missing-invoke-permission-negative, lambda__scheduler.cfn-deploy-scheduler-missing-invoke-permission-negative.

Cause 2: nominal = now + ~90 s; create_schedule(..., ScheduleExpression='at(<nominal>)', ScheduleExpressionTimezone='UTC', FlexibleTimeWindow={'Mode':'FLEXIBLE','MaximumWindowInMinutes':1}, Target=<lambda>). Record the instant the function is invoked: on AWS it is >= nominal; on the emulator it is ~57 s before nominal. Check lambda__scheduler.scheduler-lambda-flexible-time-window-invocation.

Cause 3: create_schedule(..., ScheduleExpression='at(<now+~1min>)', FlexibleTimeWindow={'Mode':'OFF'}, ActionAfterCompletion='DELETE', Target=<lambda>). After the single invocation is observed, poll get_schedule(Name=n): on AWS it becomes 404 ResourceNotFoundException; on the emulator it keeps resolving. Check lambda__scheduler.scheduler-lambda-one-time-schedule-invocation-positive.

Cause 4: create two functions A and B. create_schedule(..., Target={'Arn': A_arn, ...}); wait for an invocation of A; then update_schedule(Name=n, ScheduleExpression=..., FlexibleTimeWindow=..., State='ENABLED', Target={'Arn': B_arn, 'RoleArn': role_arn, ...}) and note the response instant T. On AWS A records nothing after T; on the emulator A records a further invocation. Check lambda__scheduler.cli-scheduler-update-schedule-changes-target.

Cause 5: the function raises on every invocation. create_schedule(..., ScheduleExpression='at(<now+~1min>)', FlexibleTimeWindow={'Mode':'OFF'}, Target={'Arn': function_arn, 'RoleArn': role_arn, 'Input': json.dumps({'marker':'retry-test'}), 'RetryPolicy': {'MaximumRetryAttempts': 2, 'MaximumEventAgeInSeconds': 86400}}). Count invocations: on AWS exactly 1; on the emulator 3. Check lambda__scheduler.scheduler-lambda-retry-policy-invocation-count.
