Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ssm.automation-start-with-alarm-in-alarm-state-rejected-even-when-poll-failure-ignored cloudwatch__ssm.automation-with-unreadable-alarm-rejected-unless-poll-failure-ignored cloudwatch__ssm.start-automation-rejected-while-its-monitored-alarm-is-in-alarm cloudwatch__ssm.monitored-alarm-entering-alarm-cancels-the-running-automation-before-its-next-step cloudwatch__ssm.automation-cancelled-when-monitored-alarm-enters-alarm-mid-run cloudwatch__ssm.automation-stops-when-watched-alarm-vanishes-unless-poll-failure-ignored cloudwatch__ssm.ssm-opsitem-action-severity-category-and-region-validated-before-delivery
Issue-Key: cloudwatch__ssm.automation-start-with-alarm-in-alarm-state-rejected-even-when-poll-failure-ignored|_rejected_with#14|AssertionError|raise AssertionError(f"expected StartAutomationExecution to be refused, it returned {answer!r}")
Issue-Key: cloudwatch__ssm.automation-with-unreadable-alarm-rejected-unless-poll-failure-ignored|_refusal#14|AssertionError|raise AssertionError(f"the start was accepted, expected a refusal: {result!r}")
Issue-Key: cloudwatch__ssm.start-automation-rejected-while-its-monitored-alarm-is-in-alarm|_refusal#13|AssertionError|raise AssertionError(f"expected a refusal, but the call succeeded: {result!r}")
Issue-Key: cloudwatch__ssm.monitored-alarm-entering-alarm-cancels-the-running-automation-before-its-next-step|_poll#15|AssertionError|raise AssertionError(f"{what}: last read {value!r}")
Issue-Key: cloudwatch__ssm.automation-cancelled-when-monitored-alarm-enters-alarm-mid-run|_until_finished#15|Timeout|time.sleep(2)
Issue-Key: cloudwatch__ssm.automation-stops-when-watched-alarm-vanishes-unless-poll-failure-ignored|_await_end#17|Timeout|time.sleep(2)
Issue-Key: cloudwatch__ssm.ssm-opsitem-action-severity-category-and-region-validated-before-delivery|_reject#7|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
Issue-Commit: a24aa969c7ede1b0812774a0cc80ce835cb01596

# cloudwatch__ssm: 3 causes — StartAutomationExecution ignores its AlarmConfiguration, a monitored alarm entering ALARM never cancels the run, and a CloudWatch ssm:opsItem action is unvalidated

> _Note: this issue was identified against the code base at commit `a24aa969c7ede1b0812774a0cc80ce835cb01596`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartAutomationExecution

## Description
Three net-new causes on the CloudWatch↔SSM boundary.

Cause 1 of 3: an AlarmConfiguration is accepted without checking the alarm.
AWS: StartAutomationExecution with AlarmConfiguration.Alarms naming an alarm that is already in ALARM, or an alarm the caller cannot read (and IgnorePollAlarmFailure false), is refused (HTTP 400) and no execution is created.
Emulator: returns 200 with a fresh AutomationExecutionId in all three shapes (alarm in ALARM, unreadable alarm, and the poll-failure-ignored variants).
Why: a runbook starts under exactly the alarm condition the caller asked it to refuse, so an alarm-gated change proceeds during an incident.
Fix: the AlarmConfiguration validation in the StartAutomationExecution handler (services/service-com-amazonaws-ssm-amazonssm/src/ops/start_automation_execution.rs), including the IgnorePollAlarmFailure branch.
Traces: cloudwatch__ssm.automation-start-with-alarm-in-alarm-state-rejected-even-when-poll-failure-ignored, cloudwatch__ssm.automation-with-unreadable-alarm-rejected-unless-poll-failure-ignored, cloudwatch__ssm.start-automation-rejected-while-its-monitored-alarm-is-in-alarm.

Cause 2 of 3: a monitored alarm entering ALARM (or vanishing) never stops the running execution.
AWS: while an execution runs, SSM polls its AlarmConfiguration; when the alarm enters ALARM the execution is cancelled before its next step, and when the alarm no longer exists the execution stops unless IgnorePollAlarmFailure is set. TriggeredAlarms then names the alarm.
Emulator: the execution keeps `AutomationExecutionStatus: 'InProgress'` with the current step still InProgress and the next step Pending, past the alarm transition; the alarm-vanishes variants are cut at the harness cap with no state change.
Why: the alarm-based safety interlock does not exist — a runbook runs on through the condition it was supposed to abort on.
Fix: the Automation engine's alarm-polling/cancellation loop.
Traces: cloudwatch__ssm.monitored-alarm-entering-alarm-cancels-the-running-automation-before-its-next-step, cloudwatch__ssm.automation-cancelled-when-monitored-alarm-enters-alarm-mid-run, cloudwatch__ssm.automation-stops-when-watched-alarm-vanishes-unless-poll-failure-ignored.

Cause 3 of 3: an alarm action that creates an OpsItem is not validated before it is stored.
AWS: PutMetricAlarm refuses an `ssm:opsitem` alarm-action ARN whose severity/category is not one of the accepted values, or whose Region is not the alarm's (HTTP 400) — the action is rejected at configuration time, before any delivery.
Emulator: PutMetricAlarm answers an empty 200 for those actions.
Why: an alarm is stored with an action AWS would never accept; the defect only surfaces (as silence) when the alarm fires.
Fix: the AlarmActions ARN validator in the CloudWatch PutMetricAlarm handler for the ssm:opsitem action form.
Traces: cloudwatch__ssm.ssm-opsitem-action-severity-category-and-region-validated-before-delivery.

## Evidence
Cause 1: 'expected StartAutomationExecution to be refused, it returned {AutomationExecutionId: a1b1f751-…, HTTPStatusCode: 200}'; 'the start was accepted, expected a refusal: {AutomationExecutionId: ded71ce0-…}'; 'expected a refusal, but the call succeeded: 3bb3569b-…'.
Cause 2: last read AutomationExecutionStatus 'InProgress' with StepExecutions [hold InProgress, after Pending] and AlarmConfiguration {'IgnorePollAlarmFailure': False, 'Alarms': [{'Name': 'sim2real-xsvc-alarm-…'}]} after the alarm entered ALARM; two variants cut at 'Check timed out after 60.0s'.
Cause 3: 'expected the call to be rejected, but it succeeded and returned {HTTPStatusCode: 200, content-length: 2}'.

## Steps to Reproduce
1. PutMetricAlarm an alarm and SetAlarmState to ALARM; call StartAutomationExecution with AlarmConfiguration naming it (with and without IgnorePollAlarmFailure). Repeat with an alarm the caller's role cannot read.
2. Start a runbook with a long first step and an AlarmConfiguration; drive the alarm to ALARM and poll GetAutomationExecution. Repeat by deleting the alarm mid-run.
3. PutMetricAlarm with an AlarmAction of the ssm:opsitem form carrying an invalid severity/category and a foreign Region.
