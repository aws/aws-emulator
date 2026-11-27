Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: fis__scheduler.disabled-schedule-makes-no-fis-call-until-enabled
Issue-Key: fis__scheduler.disabled-schedule-makes-no-fis-call-until-enabled|_await_fired#17|AssertionError|raise AssertionError(f"{what} never tagged its experiment template")
Issue-Commit: 7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8

# fis__scheduler: an ENABLED one-time EventBridge Scheduler schedule never invokes its universal FIS target (arn:aws:scheduler:::aws-sdk:fis:tagResource), so the template is never tagged and a DISABLED/ENABLED transition cannot be observed

> _Note: this issue was identified against the code base at commit `7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSchedule

## Description
Cause 1 of 1: EventBridge Scheduler accepts a schedule whose Target.Arn is the universal target arn:aws:scheduler:::aws-sdk:fis:tagResource, but the occurrence is never delivered — the FIS TagResource call is never made.
AWS: a schedule in State ENABLED with ScheduleExpression at(<timestamp>) and FlexibleTimeWindow Mode OFF invokes its target once, at the nominal instant, assuming Target.RoleArn (https://docs.aws.amazon.com/fis/latest/userguide/disable-delete-experiment-schedule.html). For a universal target the API named in the ARN is called with Target.Input as its request body, so arn:aws:scheduler:::aws-sdk:fis:tagResource with Input {"ResourceArn": <experiment template arn>, "Tags": {"fired": <marker>}} results in fis.TagResource on that template; a subsequent fis.GetExperimentTemplate then reports that tag under experimentTemplate.tags, with the key and value taken verbatim from Target.Input. A schedule in State DISABLED makes no such call; UpdateSchedule to State ENABLED with a fresh at() instant makes it. Smithy fis: TagResource is a normal tagging operation and the tag shows up in ExperimentTemplate.tags. Recorded behavior confirms the ENABLED leg tags within the schedule's lead time.
Emulator: CreateSchedule succeeds for both the ENABLED and the DISABLED leg and GetSchedule reports the right State, but nothing is ever invoked. GetExperimentTemplate on the ENABLED leg's template still reports no 'fired' tag after the full wait budget, so the check fails before it can even evaluate the DISABLED leg or the resume-by-UpdateSchedule step. The schedule is stored as configuration only; no occurrence is executed against a universal aws-sdk target.
Why: a client that wires scheduled FIS experiments (the documented way to run experiments on a cadence) sees schedules that are created, readable and correctly stated, yet nothing ever happens — and no error says so. Everything downstream of the invocation is unreachable: the target call, its retry policy, its dead-letter path, and the difference between an ENABLED and a DISABLED schedule all look identical to "nothing happened", so a user cannot distinguish a working integration from a broken one. Note this is distinct from the open issue 20261111T110051Z-cloudformation-scheduler-sqs-a-one-time-eventbridge-scheduler.md (an at() schedule firing immediately instead of at the nominal instant, for an SQS target): here the universal FIS target is never invoked at all, at any time.
Fix: the Scheduler service's occurrence-delivery component — the scheduler/timer that must execute a stored schedule at its due instant, and the universal-target (arn:aws:scheduler:::aws-sdk:<service>:<operation>) invoker that must turn Target.Arn + Target.Input into the named service call under Target.RoleArn, honouring State ENABLED/DISABLED and re-arming on UpdateSchedule.
Traces: fis__scheduler.disabled-schedule-makes-no-fis-call-until-enabled.

## Evidence
fis__scheduler.disabled-schedule-makes-no-fis-call-until-enabled:
Expected (AWS): with two wait-only FIS experiment templates and a scheduler-assumable role allowing fis:TagResource on both, CreateSchedule(State='ENABLED', ScheduleExpression='at(<now+45s>)', FlexibleTimeWindow={'Mode':'OFF'}, Target={'Arn':'arn:aws:scheduler:::aws-sdk:fis:tagResource','RoleArn':<invoker>,'Input':'{"ResourceArn":"<template arn>","Tags":{"fired":"<marker>"}}'}) causes fis.GetExperimentTemplate(id=<template>) to report tags['fired'] == <marker> shortly after the nominal instant. The DISABLED twin reports no such tag and GetSchedule reports State 'DISABLED'; after UpdateSchedule(State='ENABLED') with a fresh at() instant, its template is tagged too.
Actual (emulator): the ENABLED leg's template never acquired the tag within the 240 s budget. Replay output:
  AssertionError: the ENABLED schedule sim2real-xsvc-fisoff-on-<id> never tagged its experiment template
(the schedule name above is a per-run generated name, not a requirement). The run never reached the DISABLED-leg or resume assertions.

## Steps to Reproduce
1. Create an IAM role trusted by fis.amazonaws.com, and two FIS experiment templates holding one aws:fis:wait action (duration PT1M) with that role.
2. Create an IAM role trusted by scheduler.amazonaws.com allowing fis:TagResource on both template ARNs.
3. scheduler.CreateSchedule(Name=<on>, State='ENABLED', ScheduleExpression='at(<utc now + 45s>)', ScheduleExpressionTimezone='UTC', FlexibleTimeWindow={'Mode':'OFF'}, Target={'Arn':'arn:aws:scheduler:::aws-sdk:fis:tagResource','RoleArn':<invoker role>,'Input':'{"ResourceArn":"<template-1 arn>","Tags":{"fired":"m1"}}','RetryPolicy':{'MaximumRetryAttempts':2,'MaximumEventAgeInSeconds':300}}).
4. Poll fis.GetExperimentTemplate(id=<template-1>) for tags['fired'] == 'm1'. It never appears; AWS tags it within seconds of the nominal instant.
5. Repeat with State='DISABLED' on the second template (must stay untagged), then scheduler.UpdateSchedule(State='ENABLED') with a fresh at() instant and confirm it then tags.
