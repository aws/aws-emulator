Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__ssm.automation-status-change-events-reach-rule-queue events__ssm.automation-status-events-name-execution-and-definition-arns events__ssm.automation-failed-status-rule-gets-nothing-from-successful-run events__ssm.automation-target-missing-required-parameter-accepted-never-runs events__ssm.rule-automation-target-runs-runbook-with-event-parameter events__ssm.rule-automation-target-runs-runbook-with-event-value events__ssm.rule-target-starts-automation-under-the-assume-role-named-in-its-input events__ssm.automation-target-without-role-arn-rejected events__ssm.automation-definition-target-without-role-arn-rejected-and-with-role-starts-runbook events__ssm.automation-target-wildcard-account-arn-rejected-account-arn-runs-runbook events__ssm.command-status-change-event-detail-matches-the-sent-command events__ssm.run-command-target-without-document-version-sends-the-default-version events__ssm.run-command-target-sends-the-document-to-the-tag-targets-of-its-parameters events__ssm.run-command-target-with-document-wrapper-input-rejected-to-its-dead-letter-queue events__ssm.rule-run-command-target-runs-the-document-on-the-tagged-host events__ssm.create-ops-item-emits-an-ops-item-create-event-describing-it events__ssm.rule-opsitem-target-creates-opsitem-from-transformed-event events__ssm.rule-opsitem-target-creates-one-item-per-dedup-string
Issue-Key: events__ssm.automation-status-change-events-reach-rule-queue|_drain#22|Timeout|for msg in sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10,
Issue-Key: events__ssm.automation-status-events-name-execution-and-definition-arns|run#23|AssertionError|assert _complete_runs(got), (
Issue-Key: events__ssm.automation-failed-status-rule-gets-nothing-from-successful-run|_ids#13|Timeout|for msg in sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10,
Issue-Key: events__ssm.automation-target-missing-required-parameter-accepted-never-runs|run#36|AssertionError|raise AssertionError(
Issue-Key: events__ssm.rule-automation-target-runs-runbook-with-event-parameter|run#30|AssertionError|raise AssertionError(
Issue-Key: events__ssm.rule-automation-target-runs-runbook-with-event-value|run#42|Timeout|time.sleep(2)
Issue-Key: events__ssm.rule-target-starts-automation-under-the-assume-role-named-in-its-input|run#30|Timeout|time.sleep(2)
Issue-Key: events__ssm.automation-target-without-role-arn-rejected|_refusal#25|AssertionError|raise AssertionError(f"expected the call to be refused, but it was accepted: {result!r}")
Issue-Key: events__ssm.automation-definition-target-without-role-arn-rejected-and-with-role-starts-runbook|_refusal#24|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: events__ssm.automation-target-wildcard-account-arn-rejected-account-arn-runs-runbook|run#33|AssertionError|assert got == (
Issue-Key: events__ssm.command-status-change-event-detail-matches-the-sent-command|_until#23|Timeout|raise AssertionError(f"{what} not reached in {budget_s:.0f}s; last read {value!r}")
Issue-Key: events__ssm.run-command-target-without-document-version-sends-the-default-version|run#27|AssertionError|assert set(by_label) >= {"labelled", "<none>"} or time.monotonic() < deadline, (
Issue-Key: events__ssm.run-command-target-sends-the-document-to-the-tag-targets-of-its-parameters|run#37|Timeout|time.sleep(2)
Issue-Key: events__ssm.run-command-target-with-document-wrapper-input-rejected-to-its-dead-letter-queue|run#34|AssertionError|assert time.monotonic() < deadline, (
Issue-Key: events__ssm.rule-run-command-target-runs-the-document-on-the-tagged-host|_until#35|Timeout|raise AssertionError(f"{what} not reached in {budget_s:.0f}s; last read {value!r}")
Issue-Key: events__ssm.create-ops-item-emits-an-ops-item-create-event-describing-it|_wait_for#24|AssertionError|raise AssertionError(f"{what} never reached the queue; it received {seen!r}")
Issue-Key: events__ssm.rule-opsitem-target-creates-opsitem-from-transformed-event|run#27|AssertionError|raise AssertionError(
Issue-Key: events__ssm.rule-opsitem-target-creates-one-item-per-dedup-string|_deliver#32|Timeout|time.sleep(2)
Issue-Commit: 7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8

# events__ssm: 8 causes — SSM Automation/Run Command/OpsItem publish no state-change events, EventBridge SSM targets never act, and the role-less and wildcard-ARN target refusals diverge

> _Note: this issue was identified against the code base at commit `7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTargets

## Description
Cause 1 of 8: SSM Automation publishes no status-change events. AWS: an automation execution publishes `EC2 Automation Execution Status-change Notification` (and step-level `EC2 Automation Step Status-change Notification`) on source `aws.ssm` to the default bus, naming the execution id, the document and the status, so a status-filtered rule receives exactly the matching statuses. Recorded behavior only (documented SSM event types). Emulator: `no run delivered both a Success execution event and a step event; got []`, and the status-rule checks exhaust their 60s budget. Checks: events__ssm.automation-status-change-events-reach-rule-queue, events__ssm.automation-status-events-name-execution-and-definition-arns, events__ssm.automation-failed-status-rule-gets-nothing-from-successful-run.

Cause 2 of 8: an EventBridge target naming an SSM automation-definition ARN never starts a runbook. AWS: when the rule matches, EventBridge calls ssm:StartAutomationExecution under the target's RoleArn, passing the target's parameters (including values pulled from the event), and the execution reaches Success with those Parameters; a target whose required parameter is missing is accepted and its invocation dead-letters. Recorded behavior only. Emulator: no execution ever appears and nothing is dead-lettered — `the control runbook never finished and nothing was dead-lettered for it; dead letters so far: []`, `runbook executions did not all finish before the deadline: []`, and two further checks time out at 60s. Checks: events__ssm.automation-target-missing-required-parameter-accepted-never-runs, events__ssm.rule-automation-target-runs-runbook-with-event-parameter, events__ssm.rule-automation-target-runs-runbook-with-event-value, events__ssm.rule-target-starts-automation-under-the-assume-role-named-in-its-input.

Cause 3 of 8: PutTargets accepts an SSM automation-definition target with no RoleArn. AWS: refuses with a 400 ValidationException — an automation target must name a role to assume (Target.RoleArn is optional in Smithy, so this is a handler rule; recorded behavior for the refusal), and the same target with a role is accepted. Emulator: returns 200 `{'FailedEntryCount': 0, 'FailedEntries': []}`. Checks: events__ssm.automation-target-without-role-arn-rejected, events__ssm.automation-definition-target-without-role-arn-rejected-and-with-role-starts-runbook.

Cause 4 of 8: a wildcard-account automation-definition ARN is refused with the wrong code and message. AWS: refuses ValidationException `Parameter <arn> is not valid. Reason: Provided Arn is not in correct format.` — fixed text around the ARN taken from the request. Emulator: answers AccessDeniedException `Access to the resource <arn> is denied. Reason: Adding cross-account target is not permitted.` — wrong error class (authorization, not malformed input) and wrong message. Checks: events__ssm.automation-target-wildcard-account-arn-rejected-account-arn-runs-runbook.

Cause 5 of 8: Run Command publishes no command status-change events. AWS: SendCommand publishes `EC2 Command Status-change Notification` on `aws.ssm`, with a detail matching the sent command (command id, document, status, instance ids). Recorded behavior only. Emulator: the check exhausts its 60s budget — no event arrives. Checks: events__ssm.command-status-change-event-detail-matches-the-sent-command.

Cause 6 of 8: an EventBridge run-command target never sends a command. AWS: the target calls ssm:SendCommand under its role, resolving its RunCommandParameters tag targets, using the default document version when none is named, and dead-letters an invocation whose input wrongly wraps the document. Recorded behavior only. Emulator: `commands seen with labels [], expected one from each target`, `wiring not in effect: 0 dead letter(s), twin commands []`, plus two 60s timeouts. Checks: events__ssm.run-command-target-without-document-version-sends-the-default-version, events__ssm.run-command-target-sends-the-document-to-the-tag-targets-of-its-parameters, events__ssm.run-command-target-with-document-wrapper-input-rejected-to-its-dead-letter-queue, events__ssm.rule-run-command-target-runs-the-document-on-the-tagged-host.

Cause 7 of 8: CreateOpsItem publishes no OpsItem event. AWS: creating an OpsItem publishes `OpsItem Create` on `aws.ssm` describing the item (OpsItem id, title, source, priority). Recorded behavior only. Emulator: `the OpsItem Create event never reached the queue; it received []`. Checks: events__ssm.create-ops-item-emits-an-ops-item-create-event-describing-it.

Cause 8 of 8: an EventBridge OpsItem target creates no OpsItem. AWS: the target calls ssm:CreateOpsItem per matched event, honouring the transformed input and creating one item per distinct dedup string. Recorded behavior only. Emulator: `no OpsItem with the template's source appeared before the deadline; last dead-letter codes were []` and a 60s timeout. Checks: events__ssm.rule-opsitem-target-creates-opsitem-from-transformed-event, events__ssm.rule-opsitem-target-creates-one-item-per-dedup-string.

Why: the whole EventBridge↔SSM integration is inert in both directions — operational automation (event-driven runbooks, command fan-out, incident OpsItems) never runs and nothing is dead-lettered, so a client cannot even detect the failure; and the two refusal divergences send callers down the wrong remediation path.
Fix: the SSM event publication for automation/command/OpsItem state changes (the ssm→events bridge), the EventBridge target dispatchers for ssm automation-definition / run-command / opsitem ARNs (including dead-lettering of failed invocations), and the events PutTargets validation for SSM target ARNs (RoleArn requirement, ARN-format refusal).
Traces: the eighteen check ids listed per cause above.

## Evidence
`AssertionError: no run delivered both a Success execution event and a step event; got []`
`AssertionError: the control runbook never finished and nothing was dead-lettered for it; dead letters so far: []`
`AssertionError: expected the call to be refused, but it succeeded: {'FailedEntryCount': 0, 'FailedEntries': []}` (role-less automation target)
`AssertionError: a wildcard-account runbook ARN was answered ('AccessDeniedException', 'Access to the resource <arn> is denied. Reason: Adding cross-account target is not permitted.')` — expected ('ValidationException', 'Parameter <arn> is not valid. Reason: Provided Arn is not in correct format.')
`AssertionError: commands seen with labels [], expected one from each target` / `wiring not in effect: 0 dead letter(s), twin commands []`
`AssertionError: the OpsItem Create event never reached the queue; it received []`
`AssertionError: no OpsItem with the template's source appeared before the deadline; last dead-letter codes were []`
plus 60s timeouts on the automation/command status-change and tagged-host checks.

## Steps to Reproduce
1. ssm CreateDocument (an Automation runbook) and StartAutomationExecution; events PutRule on {source:["aws.ssm"]} + an SQS target — observe no status-change events.
2. PutTargets naming the automation-definition ARN with no RoleArn (observe 200), with a wildcard-account ARN (compare the refusal), and with a role + parameters from the event; PutEvents and look for an execution or a dead letter.
3. ssm SendCommand on a tagged host — observe no command status-change event; PutTargets naming a run-command ARN with RunCommandParameters tag targets, PutEvents, and look for a command.
4. ssm CreateOpsItem — observe no `OpsItem Create` event; PutTargets naming an opsitem target with an input transformer, PutEvents, and DescribeOpsItems.
