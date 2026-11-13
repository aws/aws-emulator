Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: scheduler__sns.scheduler-sns-target-delivers-the-notification-envelope-and-a-denied-role-withholds-it
Issue-Key: scheduler__sns.scheduler-sns-target-delivers-the-notification-envelope-and-a-denied-role-withholds-it|run#32|AssertionError|assert allowed_marker in watched, (
Issue-Commit: 23d7c3ca3e68cb70ca93342274fc19d1394baa26

# scheduler__sns: a rate(1 minute) schedule with an SNS target delivers only its FIRST occurrence — the recurrence stops, so no later occurrence ever publishes

> _Note: this issue was identified against the code base at commit `23d7c3ca3e68cb70ca93342274fc19d1394baa26`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSchedule

## Description
ONE net-new root cause of service `scheduler__sns`, grounded in the single failing check in trace_ids. The check is not marked ALREADY FILED or OWNED in the replay report, and the open scheduler__sns issues describe other surfaces (20261111T160016Z-scheduler-sns-an-sns-target-occurrence-whose-target.md and 20261112T104021Z-scheduler-sns-a-dead-letter-event-for-an.md are about the denied-role refusal and its ERROR_CODE — both of those assertions PASSED in this run; 20261109T141116Z-scheduler-sns-target-publishes-before-the-schedule-s.md is about early firing, whose assertion also passed).

=== Cause 1 of 1: an ENABLED recurring rate() schedule fires once and then never again ===
AWS: Smithy (scheduler CreateSchedule: ScheduleExpression, FlexibleTimeWindow.Mode, State) models the configuration; the firing cadence is AWS-documented behavior (EventBridge Scheduler User Guide, schedule types — a `rate(1 minute)` expression invokes the target once per minute for as long as the schedule is ENABLED and unexpired). With `FlexibleTimeWindow={'Mode':'OFF'}`, no EndDate and State=ENABLED, every minute produces one occurrence, so a subscriber watching an SNS-target schedule's topic sees a fresh notification in EVERY 60-second window, not just the first one.
Emulator: the FIRST occurrence is delivered correctly — the subscriber receives an SNS Notification envelope with the schedule's Input, substituted `<aws.scheduler.scheduled-time>`, a MessageId and a Timestamp inside the permitted lateness, and the second (denied-role) schedule's occurrence is dead-lettered with ERROR_CODE=AuthorizationError and the right SCHEDULE_ARN — and then nothing more arrives. A later 60-second observation window on the same, still-ENABLED, permitted schedule collects NO envelope at all (`markers seen []`), so the recurrence has stopped after its first fire.
Why: a recurring schedule that fires once is indistinguishable from a cron job that silently died — every periodic workload built on Scheduler (heartbeats, polling, scheduled fan-out) runs exactly once and then stalls with the schedule still reported ENABLED and no error anywhere. It also makes any differential test unsound: an absence of deliveries can no longer be attributed to a denied role, a disabled schedule or a filter, because the permitted control has stopped too.
Fix: the Scheduler occurrence dispatcher — after dispatching an occurrence of a `rate()`/`cron()` schedule, re-arm the next occurrence for as long as the schedule is ENABLED and within its StartDate/EndDate, rather than retiring the schedule after its first fire. (A fixed issue, 20261025T072339Z-scheduler-3-unowned-behavioral-causes-flexible-time-window.md, previously addressed the server-paced rate() cadence; the recurrence is not holding on the SNS-target path here.)
Traces: scheduler__sns.scheduler-sns-target-delivers-the-notification-envelope-and-a-denied-role-withholds-it.

## Evidence
scheduler__sns.scheduler-sns-target-delivers-the-notification-envelope-and-a-denied-role-withholds-it [mismatch]:
  File "check_scheduler__sns_scheduler_sns_target_delivers_the_notification_envelope_and_a_denied_role_withholds_it.py", line 178, in run — assert allowed_marker in watched
  AssertionError: the permitted schedule stopped delivering during the absence window, so the window proves nothing about the denied schedule: markers seen []
Everything before it passed: the permitted schedule's first occurrence arrived as Type='Notification' on the subscribed queue with the right TopicArn, a MessageId, the Input's marker, no unsubstituted '<aws.scheduler...' token and a Timestamp within the allowed lateness; the denied schedule's dead-letter event arrived with ERROR_CODE='AuthorizationError' and SCHEDULE_ARN = the denied schedule.
Expected: during a further 60-second window the still-ENABLED rate(1 minute) permitted schedule delivers at least one more envelope (its marker) while the denied schedule's marker never appears. Actual: no envelope for either marker — the permitted schedule delivered nothing after its first occurrence. (Marker strings, schedule names and ARNs are per-run values; the cadence is the requirement.)

## Steps to Reproduce
1. CreateTopic and subscribe an SQS queue (queue policy allowing sns.amazonaws.com); create a second queue as the dead-letter target.
2. CreateSchedule with ScheduleExpression='rate(1 minute)', ScheduleExpressionTimezone='UTC', FlexibleTimeWindow={'Mode':'OFF'}, State='ENABLED', Target={'Arn': <topic arn>, 'RoleArn': <role with sns:Publish>, 'Input': <json with a unique marker>, 'DeadLetterConfig': {'Arn': <dlq arn>}}.
3. Poll the subscribed queue: the first occurrence's notification arrives.
4. Keep polling for a further 60+ seconds without deleting or disabling the schedule — against AWS another occurrence's notification arrives every minute; against the emulator nothing arrives again. GetSchedule still reports the schedule ENABLED.
5. Re-replay scheduler__sns.scheduler-sns-target-delivers-the-notification-envelope-and-a-denied-role-withholds-it.
