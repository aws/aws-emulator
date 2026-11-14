Issue-Source: trace-sweep-divergence
Issue-Trace: iot-account-audit-config-negative-paths
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-account-audit-config-negative-paths iot-audit-task-and-finding-not-found iot-audit-task-and-finding-not-found-errors iot-audit-task-lookup-and-cancel-not-found iot-on-demand-audit-task-lifecycle
Issue-Shape-Id: com.amazonaws.iot#AWSIotService
Issue-Key: iot|StartOnDemandAuditTask|4xx->4xx|$.message
Issue-Key: iot|DescribeAuditTask|4xx->4xx|$.message
Issue-Key: iot|StartOnDemandAuditTask|2xx->2xx|$.taskId
Issue-Key: iot|DescribeAuditTask|2xx->2xx|$.auditDetails.DEVICE_CERTIFICATE_EXPIRING_CHECK.nonCompliantResourcesCount
Issue-Key: iot|DescribeAuditTask|2xx->2xx|$.auditDetails.DEVICE_CERTIFICATE_EXPIRING_CHECK.suppressedNonCompliantResourcesCount
Issue-Key: iot|DescribeAuditTask|2xx->2xx|$.auditDetails.DEVICE_CERTIFICATE_EXPIRING_CHECK.totalResourcesCount
Issue-Key: iot|DescribeAuditTask|2xx->2xx|$.auditDetails.DEVICE_CERTIFICATE_EXPIRING_CHECK.checkCompliant
Issue-Key: iot|DescribeAuditTask|2xx->2xx|$.auditDetails.DEVICE_CERTIFICATE_EXPIRING_CHECK.checkRunStatus
Issue-Key: iot|DescribeAuditTask|2xx->2xx|$.taskStatistics.canceledChecks
Issue-Key: iot|DescribeAuditTask|2xx->2xx|$.taskStatistics.compliantChecks
Issue-Key: iot|DescribeAuditTask|2xx->2xx|$.taskStatus
Issue-Commit: a6ac5f2812657951a989dc8ceb784d022fa28a4a

# iot Device Defender audit plane: 5 causes — CancelAuditTask never cancels, audit detail counters are dropped, task ids are UUIDs and two refusals are reworded

> _Note: this issue was identified against the code base at commit `a6ac5f2812657951a989dc8ceb784d022fa28a4a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartOnDemandAuditTask

## Description
Five net-new wire divergences in the IoT Device Defender audit-task plane: two refusals carry emulator-authored text, the on-demand task id has the wrong shape, DescribeAuditTask drops the per-check resource counters, and CancelAuditTask is a no-op. Five causes must be closed. The audit-CONFIGURATION clusters in this replay are already owned by 20261108T173311Z-iot-account-audit-configuration-plane-diverges-delete-leaves.md and are not re-filed here; the five below are the task-plane ones it does not carry.

Cause 1 of 5: StartOnDemandAuditTask refuses on the wrong precondition when no account audit configuration exists.
  AWS: Smithy: StartOnDemandAuditTask -> POST /audit/tasks; targetCheckNames is @required; modeled errors include InvalidRequestException (httpError 400). Recorded response when the account has no audit configuration at all: 400 with message 'Account Audit Configuration for your account have not been set up yet' — entirely fixed text, no variable part. AWS evaluates whether the account's audit configuration exists before it looks at individual checks.
  Emulator: 400 (status correct) with 'Audit check <CHECK_NAME> is not enabled for this account' — it reports a per-check enablement failure, naming a check from the request, instead of the account-level precondition.
  Why: the two refusals call for different remediations — call UpdateAccountAuditConfiguration to set the account up, versus enable one check — so a client branching on the message takes the wrong corrective action.
  Fix: the StartOnDemandAuditTask handler's precondition ordering and message (account-configuration existence first).
  Traces: iot-account-audit-config-negative-paths#task#5 (representative).

Cause 2 of 5: DescribeAuditTask's not-found message is emulator-authored.
  AWS: Smithy: DescribeAuditTask -> GET /audit/tasks/{taskId}, errors include ResourceNotFoundException (httpError 404). Recorded response: 404 with message 'Audit task with TaskId=' + <the taskId from the request> + ' was not found'. Fixed text: 'Audit task with TaskId=' and ' was not found'; variable part: the requested task id.
  Emulator: 404 ResourceNotFoundException (status and code correct) with 'Audit task <taskId> does not exist.'
  Fix: the DescribeAuditTask handler's not-found message.
  Traces: iot-account-audit-config-negative-paths#task#7 (cluster of 4: also iot-audit-task-and-finding-not-found#task#1, iot-audit-task-and-finding-not-found-errors#task#1, iot-audit-task-lookup-and-cancel-not-found#task#1).

Cause 3 of 5: StartOnDemandAuditTask returns a UUID where AWS returns a 20-character opaque audit task id.
  AWS: Smithy: StartOnDemandAuditTaskResponse.taskId targets AuditTaskId (@length min 1 max 40, pattern ^[a-zA-Z0-9\-]+$). Recorded behavior: the id is a 20-character mixed-case alphanumeric token with no hyphens (e.g. the shape of 'xYZUVCK0nLiqvBlWeanp'); the fixture asserts the id's SHAPE, not its value.
  Emulator: a canonical 36-character hyphenated UUID (e.g. 'ae6e2a34-8328-4fbf-b718-242622f2c9fe'). It satisfies the model's pattern but is a different identifier family.
  Why: a client that validates, truncates, or stores audit task ids against AWS's observed width/character class mishandles the emulator's value. The specific id is a per-run value and is not the requirement — the generated FORM is.
  Fix: the audit-task id generator in the Device Defender task state.
  Traces: iot-on-demand-audit-task-lifecycle#task#4.

Cause 4 of 5: DescribeAuditTask's auditDetails omit the per-check resource counters.
  AWS: Smithy: DescribeAuditTask -> output auditDetails (AuditDetails map of check name to AuditCheckDetails). Recorded response for a completed check: the entry carries totalResourcesCount, nonCompliantResourcesCount and suppressedNonCompliantResourcesCount, all 0 when nothing was found.
  Emulator: those three members are absent from the check entry.
  Why: a client computing a compliance ratio or deciding 'nothing non-compliant' reads null where AWS always reports a number, so it either divides by nothing or treats a clean audit as unknown.
  Fix: the DescribeAuditTask serializer / the audit-check detail record (emit the counters, zero-valued when the check found nothing).
  Traces: iot-on-demand-audit-task-lifecycle#task#5.

Cause 5 of 5: CancelAuditTask does not cancel — the task completes anyway.
  AWS: Smithy: CancelAuditTask -> PUT /audit/tasks/{taskId}/cancel (output empty); DescribeAuditTask output taskStatus (AuditTaskStatus enum), taskStatistics, auditDetails. Recorded response after cancelling a running on-demand task: taskStatus 'CANCELED', the cancelled check's checkRunStatus 'CANCELED' with checkCompliant null, taskStatistics.canceledChecks 1 and compliantChecks 0.
  Emulator: taskStatus 'COMPLETED', checkRunStatus 'COMPLETED_COMPLIANT', checkCompliant true, canceledChecks 0, compliantChecks 1 — and the fixture's poll gives up because 'COMPLETED' is a terminal state it must never reach.
  Why: cancellation is silently ignored: the call returns success while the audit keeps running to completion, so a client that cancels to stop an audit (cost, noise, a bad check) sees a finished audit with compliance verdicts AWS would never have produced.
  Fix: the CancelAuditTask handler must mark the task and its in-flight checks CANCELED in the audit-task state, and the audit runner must honour that flag instead of settling the task COMPLETED.
  Traces: iot-on-demand-audit-task-lifecycle#task#9.

## Evidence
Wire corpus simparity-wire-aws-v1 replayed against the emulator built from this checkout. All five clusters are verdict 'mismatch' with empty owned_by and empty stub_path.

Cause 1 — iot-account-audit-config-negative-paths#task#5 (StartOnDemandAuditTask), status 400 as expected:
  $.message: expected 'Account Audit Configuration for your account have not been set up yet', got 'Audit check IOT_POLICY_OVERLY_PERMISSIVE_CHECK is not enabled for this account'

Cause 2 — iot-account-audit-config-negative-paths#task#7 (DescribeAuditTask), status 404 as expected:
  $.message: expected 'Audit task with TaskId=<taskId> was not found', got 'Audit task <taskId> does not exist.'
  also affected: iot-audit-task-and-finding-not-found#task#1, iot-audit-task-and-finding-not-found-errors#task#1, iot-audit-task-lookup-and-cancel-not-found#task#1

Cause 3 — iot-on-demand-audit-task-lifecycle#task#4 (StartOnDemandAuditTask), status 200:
  $.taskId: shape mismatch — recorded shape is a 20-char mixed-case alphanumeric token, got 'ae6e2a34-8328-4fbf-b718-242622f2c9fe' (hyphenated UUID)

Cause 4 — iot-on-demand-audit-task-lifecycle#task#5 (DescribeAuditTask), status 200:
  $.auditDetails.<CHECK>.totalResourcesCount: missing (expected 0)
  $.auditDetails.<CHECK>.nonCompliantResourcesCount: missing (expected 0)
  $.auditDetails.<CHECK>.suppressedNonCompliantResourcesCount: missing (expected 0)

Cause 5 — iot-on-demand-audit-task-lifecycle#task#9 (DescribeAuditTask after CancelAuditTask), status 200:
  $.taskStatus: expected 'CANCELED', got 'COMPLETED' (poll: 'COMPLETED' is in fail_on after 2 attempts)
  $.auditDetails.<CHECK>.checkRunStatus: expected 'CANCELED', got 'COMPLETED_COMPLIANT'
  $.auditDetails.<CHECK>.checkCompliant: expected None, got True
  $.taskStatistics.canceledChecks: expected 1, got 0
  $.taskStatistics.compliantChecks: expected 0, got 1

The check name, the task ids and the account id above are per-run values; the requirements are the statuses, the fixed text of each message plus the request value it interpolates, the id's generated form, the presence of the zero-valued counters, and the CANCELED terminal state with its statistics.

Smithy consulted: services/service-com-amazonaws-iot-awsiotservice/smithy-model/com-amazonaws-iot-awsiotservice.json — StartOnDemandAuditTask (POST /audit/tasks; targetCheckNames required; InvalidRequestException 400; taskId -> AuditTaskId @length 1..40 pattern ^[a-zA-Z0-9\-]+$), DescribeAuditTask (GET /audit/tasks/{taskId}; taskStatus/taskStatistics/auditDetails; ResourceNotFoundException 404), CancelAuditTask (PUT /audit/tasks/{taskId}/cancel; ResourceNotFoundException 404).

## Steps to Reproduce
1. Boot the emulator built from this checkout and point an iot client at it.
2. Cause 1: on an account with NO audit configuration, StartOnDemandAuditTask(targetCheckNames=['IOT_POLICY_OVERLY_PERMISSIVE_CHECK']) and read Error.Message — expect the account-level 'Account Audit Configuration for your account have not been set up yet'.
3. Cause 2: DescribeAuditTask(taskId=<an id never issued>) and read Error.Message — expect 'Audit task with TaskId=<id> was not found'.
4. Cause 3: UpdateAccountAuditConfiguration to set the account up, then StartOnDemandAuditTask and inspect the returned taskId's length and character class.
5. Cause 4: DescribeAuditTask on that task once the check has run and read auditDetails.<CHECK> — expect totalResourcesCount, nonCompliantResourcesCount and suppressedNonCompliantResourcesCount present (0 when clean).
6. Cause 5: start another on-demand task, CancelAuditTask(taskId) immediately, then poll DescribeAuditTask — expect taskStatus CANCELED, checkRunStatus CANCELED, checkCompliant null, taskStatistics.canceledChecks 1 and compliantChecks 0; it must never reach COMPLETED.
7. Re-replay the representative step iot-account-audit-config-negative-paths#task#5 and every fixture#step in trace_ids.
