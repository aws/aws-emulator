Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: application-autoscaling__ecs.suspended-scaling-dimensions-are-independent-not-one-switch
Issue-Key: application-autoscaling__ecs.suspended-scaling-dimensions-are-independent-not-one-switch|run#38|AssertionError|assert _alarm_state(cw, estates['subject']['in_alarm']) == 'ALARM', (
Issue-Commit: 96db52f5b4a4b596ab227bd8becc0930845733a7

# application-autoscaling__ecs: a CloudWatch scale-in alarm does not stay in ALARM while its metric is absent, so DynamicScalingInSuspended cannot be observed

> _Note: this issue was identified against the code base at commit `96db52f5b4a4b596ab227bd8becc0930845733a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutScalingPolicy

## Description
Cause 1 of 1: the alarm backing a suspended scale-in policy leaves ALARM during the metric-absence window.
AWS: a CloudWatch alarm attached to an Application Auto Scaling step/target-tracking scale-in policy holds its ALARM state while no new datapoints arrive (missing data is treated per the alarm's TreatMissingData setting, which for these alarms keeps the existing state rather than re-evaluating to OK). With the alarm continuously in ALARM, Application Auto Scaling's suspended-state flags are independent switches: with DynamicScalingInSuspended=true the in-alarm scale-in policy performs no action while scale-out and scheduled scaling stay live, each suspension flag applying only to its own dimension (recorded behavior; the application-autoscaling model exposes SuspendedState.DynamicScalingInSuspended / DynamicScalingOutSuspended / ScheduledScalingSuspended as separate booleans).
Emulator: the subject's scale-in alarm transitions out of ALARM during the absence window, so the capacity staying unchanged proves nothing about the suspension flag and the check's third leg cannot be graded.
Why: a client cannot distinguish “scale-in was suppressed because DynamicScalingInSuspended is set” from “the alarm stopped asking”. Any suspension workflow (maintenance freeze, deploy window) that is validated by holding an alarm in ALARM and asserting no activity is unverifiable, and alarm state generally is unreliable whenever a metric stops reporting.
Fix: alarm state evaluation on absent datapoints in the CloudWatch alarm evaluator (the alarm state machine in services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/src/) — honour TreatMissingData/state retention instead of leaving ALARM, and keep Application Auto Scaling's per-dimension suspension independent.
Traces: check application-autoscaling__ecs.suspended-scaling-dimensions-are-independent-not-one-switch.

## Evidence
AssertionError: leg 3: the subject's scale-in alarm '<alarm-name>' left ALARM during the absence window, so an unchanged capacity would prove nothing about DynamicScalingInSuspended. Expected _alarm_state(cw, <scale-in alarm>) == 'ALARM' throughout the window in which no datapoints are published; observed a state other than ALARM.

## Steps to Reproduce
Replay check application-autoscaling__ecs.suspended-scaling-dimensions-are-independent-not-one-switch: register an ECS service as a scalable target on ecs:service:DesiredCount, attach step scaling policies with CloudWatch alarms on a custom metric, drive the scale-in alarm into ALARM by publishing breaching datapoints, set SuspendedState.DynamicScalingInSuspended=true, then stop publishing and assert the scale-in alarm is still in ALARM while capacity stays unchanged.
