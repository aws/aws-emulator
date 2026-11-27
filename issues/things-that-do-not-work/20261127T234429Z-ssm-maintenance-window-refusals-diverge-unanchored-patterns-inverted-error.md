Issue-Source: trace-sweep-divergence
Issue-Trace: maintenance-window-task-id-pattern
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: maintenance-window-task-id-pattern ssm-maintenance-window-task-id-length-bounds ssm-cancel-maintenance-window-execution-id-length-bounds ssm-cancel-maintenance-window-execution-id-pattern ssm-delete-maintenance-window-id-pattern-and-legal-neighbour ssm-maintenance-window-delete-window-id-length ssm-deregister-window-target-id-fixed-length ssm-maintenance-window-deregister-target-id-pattern ssm-describe-task-invocations-filter-value-length-bounds ssm-execution-task-invocations-task-id-fixed-length ssm-window-task-invocations-task-id-pattern ssm-maintenance-window-automation-task-document-version-grammar ssm-maintenance-window-run-command-document-hash-type-outside-enum-refused ssm-maintenance-window-invocation-id-pattern ssm-window-task-invocation-id-length-bounds ssm-maintenance-window-name-length-bounds ssm-maintenance-window-schedule-length-bound ssm-maintenance-window-schedule-max-results-floor ssm-maintenance-window-schedule-resource-type-values
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm|DeregisterTaskFromMaintenanceWindow|4xx->4xx|$.message
Issue-Key: ssm|GetMaintenanceWindowTask|4xx->4xx|$.Message
Issue-Key: ssm|CancelMaintenanceWindowExecution|4xx->4xx|$.message
Issue-Key: ssm|DeleteMaintenanceWindow|4xx->4xx|$.message
Issue-Key: ssm|DeregisterTargetFromMaintenanceWindow|4xx->4xx|$.message
Issue-Key: ssm|DescribeMaintenanceWindowExecutionTaskInvocations|4xx->4xx|$.message
Issue-Key: ssm|RegisterTaskWithMaintenanceWindow|4xx->4xx|$.message
Issue-Key: ssm|GetMaintenanceWindowExecutionTaskInvocation|4xx->4xx|$.message
Issue-Key: ssm|CreateMaintenanceWindow|4xx->4xx|$.message
Issue-Key: ssm|DescribeMaintenanceWindowSchedule|4xx->4xx|$.message
Issue-Commit: 255279eeed7e5207bd6ac2895e89724e049cf5ef

# Maintenance window refusals diverge: unanchored patterns, inverted error order, re-worded messages

> _Note: this issue was identified against the code base at commit `255279eeed7e5207bd6ac2895e89724e049cf5ef`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateMaintenanceWindow

## Description
Ten net-new causes on the Maintenance Windows plane. Shared fault: the refusal body for a malformed window/target/task identifier is not AWS's — the regular expression is quoted without its anchors, multiple validation errors come out in the wrong order, the rejected value is echoed where AWS omits it, and two messages are emulator-authored sentences.

AWS message rule for all pattern/length causes below: the body is "<n> validation error(s) detected: Value at '<member path>' failed to satisfy constraint: Member must satisfy regular expression pattern: <anchored pattern>[; Value at '<path>' failed to satisfy constraint: Member must have length <bound>]" — fixed text is the template and the constraint sentences; variables are the member path, the pattern and the limit. AWS puts the pattern clause first and does NOT quote the submitted value for these members.

Cause 1 of 10: DeregisterTaskFromMaintenanceWindow.
AWS: pattern quoted as ^[0-9a-fA-F]{8}\\-[0-9a-fA-F]{4}\\-[0-9a-fA-F]{4}\\-[0-9a-fA-F]{4}\\-[0-9a-fA-F]{12}$, no value echoed.
Emulator: un-anchored pattern and echoes the windowTaskId.
Fix: the validation message renderer for windowTaskId.
Traces: maintenance-window-task-id-pattern#task#6.

Cause 2 of 10: GetMaintenanceWindowTask's not-found message is emulator-authored.
AWS: 400 with Message "Maintenance window task <windowTaskId> does not exist" — the variable is the requested task id only.
Emulator: "Maintenance window <windowId> has no task <windowTaskId>".
Why: clients matching AWS's sentence do not recognise the refusal; the emulator also discloses the window id AWS leaves out. Related open issue 20261125T023529Z-ssm-the-doesnotexistexception-message-for-a-windowtaskid-that.md describes this family but does not own this cluster.
Fix: the GetMaintenanceWindowTask DoesNotExistException message.
Traces: maintenance-window-task-id-pattern#task#9.

Cause 3 of 10: CancelMaintenanceWindowExecution.
AWS: two errors, pattern clause first (anchored), then "Member must have length greater than or equal to 36", no value echoed.
Emulator: length clause first, un-anchored pattern, value echoed in both clauses.
Fix: the validation walk order plus the message renderer.
Traces: ssm-cancel-maintenance-window-execution-id-length-bounds#task#2.

Cause 4 of 10: DeleteMaintenanceWindow.
AWS: "Member must satisfy regular expression pattern: ^mw-[0-9a-f]{17}$", no value echoed.
Emulator: un-anchored 'mw-[0-9a-f]{17}' and echoes the windowId.
Fix: as cause 1, for windowId.
Traces: ssm-delete-maintenance-window-id-pattern-and-legal-neighbour#task#2.

Cause 5 of 10: DeregisterTargetFromMaintenanceWindow.
AWS: pattern clause first (anchored) then the length clause, no value echoed.
Emulator: order inverted, pattern un-anchored, value echoed.
Fix: as cause 3, for windowTargetId.
Traces: ssm-deregister-window-target-id-fixed-length#task#3.

Cause 6 of 10: DescribeMaintenanceWindowExecutionTaskInvocations renders a list-member constraint at the wrong level.
AWS: "1 validation error detected: Value at 'filters.1.member.values' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 256, Member must have length greater than or equal to 1]".
Emulator: reports 'filters.1.member.values.1.member' with one constraint and a truncated echoed value ('... [257 bytes]').
Fix: the validation error renderer for list members.
Traces: ssm-describe-task-invocations-filter-value-length-bounds#task#3.

Cause 7 of 10: RegisterTaskWithMaintenanceWindow's documentVersion grammar is short two alternatives.
AWS: "Member must satisfy regular expression pattern: ([$]LATEST|[$]DEFAULT|[$]APPROVED|[$]PENDING_REVIEW|^[1-9][0-9]*$)" and no value echoed.
Emulator: quotes ([$]LATEST|[$]DEFAULT|^[1-9][0-9]*$) and echoes the value — so $APPROVED and $PENDING_REVIEW are refused as malformed.
Why: a task registration AWS accepts is refused.
Fix: the automation documentVersion constraint for window tasks plus the message.
Traces: ssm-maintenance-window-automation-task-document-version-grammar#task#6.

Cause 8 of 10: GetMaintenanceWindowExecutionTaskInvocation.
AWS: anchored invocationId pattern, no value echoed.
Emulator: un-anchored, value echoed.
Fix: as cause 1, for invocationId.
Traces: ssm-maintenance-window-invocation-id-pattern#task#1.

Cause 9 of 10: CreateMaintenanceWindow name/schedule refusals.
AWS: pattern clause first — "Member must satisfy regular expression pattern: ^[a-zA-Z0-9_\\-.]{3,128}$" — then the length clause, no value echoed.
Emulator: length first, un-anchored pattern, value echoed.
Fix: as cause 3, for name and schedule.
Traces: ssm-maintenance-window-name-length-bounds#task#1.

Cause 10 of 10: DescribeMaintenanceWindowSchedule's maxResults refusal echoes the value.
AWS: "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value greater than or equal to 1".
Emulator: inserts "Value '0' at 'maxResults'".
Fix: the validation message renderer for ranged members.
Traces: ssm-maintenance-window-schedule-max-results-floor#task#3.

## Evidence
Replay of simparity-wire-aws-v1; every cause is a $.message / $.Message value_mismatch at status 400->400.
Cause 1 maintenance-window-task-id-pattern#task#6: expected anchored ^...$ pattern and no value; got un-anchored pattern with "Value 'g1b2c3d4-0000-4000-8000-000000000000'".
Cause 2 maintenance-window-task-id-pattern#task#9: $.Message expected 'Maintenance window task <id> does not exist', got 'Maintenance window <windowId> has no task <id>'.
Cause 3 ssm-cancel-maintenance-window-execution-id-length-bounds#task#2: clause order inverted, pattern un-anchored, value echoed.
Cause 4 ssm-delete-maintenance-window-id-pattern-and-legal-neighbour#task#2: expected '^mw-[0-9a-f]{17}$', got 'mw-[0-9a-f]{17}' plus value echo.
Cause 5 ssm-deregister-window-target-id-fixed-length#task#3: as cause 3 for windowTargetId.
Cause 6 ssm-describe-task-invocations-filter-value-length-bounds#task#3: path 'filters.1.member.values' vs '...values.1.member', single constraint, '... [257 bytes]' truncation.
Cause 7 ssm-maintenance-window-automation-task-document-version-grammar#task#6: emulator pattern omits [$]APPROVED|[$]PENDING_REVIEW.
Cause 8 ssm-maintenance-window-invocation-id-pattern#task#1: un-anchored pattern + value echo.
Cause 9 ssm-maintenance-window-name-length-bounds#task#1: clause order inverted, un-anchored pattern, value echoed.
Cause 10 ssm-maintenance-window-schedule-max-results-floor#task#3: value '0' echoed.

## Steps to Reproduce
1. Boot the emulator and replay the corpus.
2. Replay each listed fixture and compare the 400 body at the named step with the recorded AWS body.
3. Representative: fixture maintenance-window-task-id-pattern step task#6 (DeregisterTaskFromMaintenanceWindow with a non-hex WindowTaskId).
