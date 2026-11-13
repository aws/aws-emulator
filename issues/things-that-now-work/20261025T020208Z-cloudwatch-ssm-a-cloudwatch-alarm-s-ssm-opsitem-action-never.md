Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ssm.xsvc-cloudwatch-3-alarm-ssm-opsitem-action-creates-opsitem-and-disabled-twin-blocked
Issue-Key: cloudwatch__ssm.xsvc-cloudwatch-3-alarm-ssm-opsitem-action-creates-opsitem-and-disabled-twin-blocked|_poll#13|Timeout|time.sleep(5.0)
Issue-Commit: d26de989ce4778e024797c8509c4713b425f0550

# A CloudWatch alarm's SSM OpsItem action never creates an OpsItem, and the actions-disabled twin cannot be shown to be blocked (1 cause)

> _Note: this issue was identified against the code base at commit `d26de989ce4778e024797c8509c4713b425f0550`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricAlarm

## Description
One net-new root cause.

Cause 1 of 1: the ssm:opsitem alarm action is never dispatched.
AWS: an alarm whose AlarmActions contain an `arn:aws:ssm:<region>:<account>:opsitem:<severity>` action creates a Systems Manager OpsItem when it transitions to ALARM — discoverable through DescribeOpsItems with the alarm as its source — while an otherwise identical alarm with ActionsEnabled=false transitions but creates none. The pair is what separates "action dispatched" from "action suppressed". Recorded behavior: one OpsItem for the enabled alarm and none for the twin.
Emulator: the check drives both alarms and polls DescribeOpsItems, and is killed at the harness 60 s cap — no OpsItem is ever created, so the disabled twin's "blocked" arm has no positive control and the suppression cannot be distinguished from a non-functional action path. This is the SSM-target instance of the alarm action-dispatch engine not running (filed for service `cloudwatch`); that issue does not carry this replay key.
Why: OpsItem creation is how CloudWatch alarms raise operational work items; with it never dispatched, an alarm that should open an incident opens nothing, and an operator relying on OpsCenter sees an empty queue while the alarm is breaching.
Fix: the ssm:opsitem alarm-action target in the cloudwatch alarm action dispatch path, and the ActionsEnabled gate in front of it.
Traces: cloudwatch__ssm.xsvc-cloudwatch-3-alarm-ssm-opsitem-action-creates-opsitem-and-disabled-twin-blocked.

## Evidence
"Error: Check timed out after 60.0s" (cloudwatch__ssm.xsvc-cloudwatch-3-alarm-ssm-opsitem-action-creates-opsitem-and-disabled-twin-blocked) — the check polls DescribeOpsItems for the OpsItem the enabled alarm should have created and never sees one.

## Steps to Reproduce
1. PutMetricAlarm(AlarmName=a, AlarmActions=['arn:aws:ssm:<region>:<account>:opsitem:3'], ActionsEnabled=true) and an identical twin with ActionsEnabled=false.
2. PutMetricData breaching datapoints for both metrics.
3. Poll DescribeOpsItems filtered to the alarm source -> no OpsItem is ever created for either alarm; the check hangs to the 60 s cap.
