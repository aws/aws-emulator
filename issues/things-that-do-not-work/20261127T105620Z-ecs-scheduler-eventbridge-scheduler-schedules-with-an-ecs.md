Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs__scheduler.cron-schedule-launches-a-new-ecs-task-each-minute-after-its-start-date ecs__scheduler.disabled-schedule-launches-no-ecs-task-until-it-is-enabled ecs__scheduler.flexible-window-cron-schedule-launches-its-ecs-task-inside-the-window ecs__scheduler.one-time-schedule-launches-its-ecs-task-exactly-once
Issue-Key: ecs__scheduler.cron-schedule-launches-a-new-ecs-task-each-minute-after-its-start-date|_wait_for#25|AssertionError|raise AssertionError(f"{what} did not happen within {budget_s:.0f}s")
Issue-Key: ecs__scheduler.disabled-schedule-launches-no-ecs-task-until-it-is-enabled|_wait_for#27|AssertionError|raise AssertionError(f"{what} did not happen within {budget_s:.0f}s")
Issue-Key: ecs__scheduler.flexible-window-cron-schedule-launches-its-ecs-task-inside-the-window|_wait_for#30|AssertionError|raise AssertionError(f"{what} did not happen within {budget_s:.0f}s")
Issue-Key: ecs__scheduler.one-time-schedule-launches-its-ecs-task-exactly-once|_wait_for#26|AssertionError|raise AssertionError(f"{what} did not happen within {budget_s:.0f}s")
Issue-Commit: 96db52f5b4a4b596ab227bd8becc0930845733a7

# ecs__scheduler: EventBridge Scheduler schedules with an ECS RunTask target never launch a task — cron, one-time and flexible-window occurrences all fail to fire

> _Note: this issue was identified against the code base at commit `96db52f5b4a4b596ab227bd8becc0930845733a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSchedule

## Description
Cause 1 of 1: a Scheduler schedule whose target is the ECS RunTask universal target never invokes it, so no task is ever launched.
AWS: after CreateSchedule with State=ENABLED, Scheduler invokes the target at each occurrence of its ScheduleExpression: a `cron(...)`/`rate(...)` schedule launches a new ECS task at each occurrence once the StartDate has passed; an `at(...)` schedule launches exactly one task at the nominal instant; a schedule with FlexibleTimeWindow mode FLEXIBLE launches once inside the declared window; a schedule created with State=DISABLED launches nothing until UpdateSchedule sets State=ENABLED, after which it launches on its occurrences like any enabled schedule. The launched tasks are observable through ecs.ListTasks/DescribeTasks for the target cluster and task definition (recorded behavior).
Emulator: no task ever appears for any of the four shapes — every check's bounded wait for the launch(es) expires (240s–420s budgets) with nothing launched, including the ENABLED control schedule of the disabled-schedule check. The common factor is the Scheduler occurrence/invocation engine for the ECS RunTask target, not one expression form.
Why: any client built on Scheduler-driven ECS batch work sees schedules that are created, describable and “ENABLED” but completely inert, so nothing downstream of the occurrence (task launch, task completion, the one-shot/once-per-window guarantees, or the enable/disable transition) can be modelled or tested. The disabled case even passes vacuously in real use: “no task launched” is indistinguishable from the broken enabled case.
Fix: the schedule occurrence/invocation path of the Scheduler service and its ECS RunTask universal-target invoker (the schedule-firing component in services/service-com-amazonaws-scheduler-awschronosservice/src/, which must call ECS RunTask and create real task state).
Related open issues to check against (neither was marked as owning these checks): 20261111T110051Z-cloudformation-scheduler-sqs-a-one-time-eventbridge-scheduler.md (one-time at() delivering immediately for an SQS target) and 20261121T130922Z-scheduler-2-net-new-behavioral-causes-the-dead.md.
Traces: checks ecs__scheduler.cron-schedule-launches-a-new-ecs-task-each-minute-after-its-start-date, ecs__scheduler.disabled-schedule-launches-no-ecs-task-until-it-is-enabled, ecs__scheduler.flexible-window-cron-schedule-launches-its-ecs-task-inside-the-window, ecs__scheduler.one-time-schedule-launches-its-ecs-task-exactly-once.

## Evidence
AssertionError: two launches by the cron schedule did not happen within 300s (cron-schedule-launches-a-new-ecs-task-each-minute-after-its-start-date).
AssertionError: two launches by the enabled control schedule did not happen within 240s (disabled-schedule-launches-no-ecs-task-until-it-is-enabled).
AssertionError: the flexible schedule's launch did not happen within 420s (flexible-window-cron-schedule-launches-its-ecs-task-inside-the-window).
AssertionError: the one-time schedule's launch did not happen within 300s (one-time-schedule-launches-its-ecs-task-exactly-once).
Expected in each case: one or more ECS tasks launched by the schedule and visible via ListTasks/DescribeTasks for the target cluster/task definition within the check's budget.

## Steps to Reproduce
For each listed check: register an ECS task definition and cluster, create an IAM role Scheduler can assume with ecs:RunTask, then scheduler.CreateSchedule with Target = the ECS RunTask universal target (Arn 'arn:aws:scheduler:::aws-sdk:ecs:runTask' style target with the cluster/task definition input) and
1. ScheduleExpression 'cron(* * * * ? *)' with a StartDate just ahead — poll for two launches (300s).
2. State='DISABLED' plus an ENABLED control schedule — poll the control for two launches (240s), then UpdateSchedule State='ENABLED'.
3. FlexibleTimeWindow Mode='FLEXIBLE' with a window — poll for the single launch (420s).
4. ScheduleExpression 'at(<instant>)' — poll for exactly one launch (300s).
Observe that ListTasks for the target cluster never reports any task.
