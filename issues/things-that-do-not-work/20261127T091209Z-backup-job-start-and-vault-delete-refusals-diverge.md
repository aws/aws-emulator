Issue-Source: trace-sweep-divergence
Issue-Trace: backup-start-backup-job-omitted-role-refused-as-access-denied
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: backup-start-backup-job-omitted-role-refused-as-access-denied backup-restore-job-refusals backup-scan-job-start-refusals backup-start-scan-job-required-member-validation backup-report-job-plan-name-grammar-refusals backup-report-job-start-refusals
Issue-Shape-Id: com.amazonaws.backup#CryoControllerUserManager
Issue-Key: backup|StartBackupJob|4xx->4xx|$.Message
Issue-Key: backup|StartBackupJob|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: backup|StartBackupJob|4xx->4xx|status
Issue-Key: backup|StartBackupJob|4xx->2xx|$.Code
Issue-Key: backup|StartBackupJob|4xx->2xx|$.Message
Issue-Key: backup|StartBackupJob|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: backup|StartBackupJob|4xx->2xx|status
Issue-Key: backup|DeleteBackupVault|2xx->4xx|status
Issue-Key: backup|StartRestoreJob|4xx->4xx|$.Code
Issue-Key: backup|StartRestoreJob|4xx->4xx|$.Message
Issue-Key: backup|StartRestoreJob|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: backup|StartScanJob|4xx->4xx|$.message
Issue-Key: backup|StartScanJob|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: backup|StartReportJob|4xx->4xx|$.Message
Issue-Commit: da5b5c82f86ff32a5f1d8fa609cf9d0b9820cd03

# Backup job-start and vault-delete refusals diverge: absent Code/Message, wrong status class, and an unchecked backup role (7 causes)

> _Note: this issue was identified against the code base at commit `da5b5c82f86ff32a5f1d8fa609cf9d0b9820cd03`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartBackupJob

## Description
Seven distinct root causes across the remaining net-new wire divergences of the `backup` service (StartBackupJob, DeleteBackupVault, StartRestoreJob, StartScanJob, StartReportJob), all in services/service-com-amazonaws-backup-cryocontrollerusermanager, bundled as one work item. Each must be closed separately. None of these clusters carries stub_path or owned_by in the replay report. Adjacent open issues do NOT cover them: 20261029T175130Z-backup-aws-behavior-... (20261029T175130Z-backup-aws-backup-vault-lock-and-tiering-refusals-use.md) files StartBackupJob authorization for an IDENTITY policy with an explicit deny and for a malformed ResourceArn, not the omitted-role 403 of cause 1 nor the role-permission ERROR_3020 refusal of cause 2; 20261124T140401Z-backup-events-2-net-new-causes-no-backup.md is about EventBridge job-state events.

The shared fault: these job-start handlers reach the right HTTP status only sometimes, and in no case do they render AWS Backup's refusal envelope — Smithy: InvalidParameterValueException / MissingParameterValueException / ResourceNotFoundException / InvalidRequestException all declare members {Code, Message, Type, Context} and all map to HTTP 400 — while two of them accept a call AWS refuses.

=== Cause 1 of 7: StartBackupJob with IamRoleArn omitted answers 400 MissingParameterValue with no message, where AWS refuses 403 with an authorization sentence ===
AWS: Smithy: StartBackupJob (PUT /backup-jobs) requires BackupVaultName, ResourceArn and IamRoleArn, and models InvalidParameterValueException/InvalidRequestException/LimitExceededException/MissingParameterValueException/ResourceNotFoundException (400) plus ServiceUnavailableException (500) — AccessDeniedException is NOT in that set; it is the service-common authorization refusal. Recorded behavior: when no role is supplied, AWS authorizes first and refuses with HTTP 403 and Message 'Insufficient privileges to perform this action.' — the whole sentence is fixed text, nothing from the request appears in it — served by an error shape other than MissingParameterValueException.
Emulator: answers HTTP 400 with x-amzn-errortype MissingParameterValueException and $.Message missing entirely (got None).
Why: the status class itself is wrong (4xx-authorization vs 4xx-validation): a client that branches on 403 to re-acquire credentials or surface a permissions error instead sees a parameter error with an empty body, and gets no message to show the operator.
Fix: the pre-validation authorization step of the StartBackupJob handler — the omitted-role path must be refused in the AccessDenied class with HTTP 403 and AWS's fixed sentence, before required-member validation runs.
Traces: backup-start-backup-job-omitted-role-refused-as-access-denied#task#4.

=== Cause 2 of 7: StartBackupJob ACCEPTS a role that lacks backup permissions — AWS refuses it ERROR_3020 ===
AWS: Smithy: the modeled 400 refusals for StartBackupJob all carry {Code, Message, Type, Context}; the permission decision itself is not expressed in the model, so: recorded behavior — a role that cannot perform the backup is refused HTTP 400 with Code 'ERROR_3020' and Message 'IAM Role does not have sufficient permissions to execute the backup' (fixed text, no request values interpolated), and no backup job is created.
Emulator: answers HTTP 200 and hands back a job — expected 400, got 200; $.Code, $.Message and the x-amzn-errortype header are all absent because there is no error at all. The handler never evaluates what the supplied role can do.
Why: this is an overly-permissive divergence. A client or least-privilege test that verifies an under-privileged role cannot start a backup observes a successful job and a recovery point where AWS creates nothing, so permission regressions pass silently; downstream state (recovery points, job listings, vault emptiness — see cause 3) is then populated with resources AWS would never have created.
Fix: the role-permission evaluation in the StartBackupJob handler — it must refuse with Code ERROR_3020 and AWS's fixed sentence before creating any job state.
Traces: backup-start-backup-job-omitted-role-refused-as-access-denied#task#5.

=== Cause 3 of 7: DeleteBackupVault refuses with 400 where AWS deletes the vault and answers 200 ===
AWS: Smithy: DeleteBackupVault is DELETE /backup-vaults/{BackupVaultName}, output Unit, success code 200, errors InvalidParameterValueException/InvalidRequestException/MissingParameterValueException/ResourceNotFoundException (400) and ServiceUnavailableException (500). Recorded behavior: the teardown delete of the probe's vault succeeds with HTTP 200 and an empty body.
Emulator: answers HTTP 400. Hypothesis: the vault is judged non-empty because cause 2 let a backup job (and its recovery point) be created in it, so the handler takes its "vault still contains recovery points" refusal branch; the requirement either way is that a vault AWS considers deletable deletes with 200 and an empty body.
Why: teardown of any fixture or client workflow cannot complete — the vault leaks, and a subsequent create of the same name collides, so a whole test or provisioning run is left wedged on a call AWS completes.
Fix: the deletability precondition of the DeleteBackupVault handler (and the state it reads, which must not contain the job created in cause 2).
Traces: backup-start-backup-job-omitted-role-refused-as-access-denied#teardown#6.

=== Cause 4 of 7: StartRestoreJob with no IAM role is reported as a missing recovery point ===
AWS: Smithy: StartRestoreJob (PUT /restore-jobs) takes IamRoleArn (optional in the model) and models InvalidParameterValueException/InvalidRequestException/MissingParameterValueException/ResourceNotFoundException (400) with {Code, Message, Type, Context}. Recorded behavior: when the role is absent the refusal is 400 with Code 'ERROR_4003' and Message 'IAM Role is null' — fixed text, no request values — carried by an error shape other than InvalidParameterValueException (per the model, an absent required value is the MissingParameterValue class).
Emulator: answers 400 InvalidParameterValueException with Message 'Cannot find recovery point' and no Code — it resolves the recovery point before noticing the role is missing.
Why: the caller is sent to fix a recovery point that is not the problem, error-type and Code-based branching both miss, and the real cause (no role) is never reported.
Fix: the required-parameter validation and its ordering in the StartRestoreJob handler — the null-role refusal must precede the recovery-point lookup and carry Code ERROR_4003 with AWS's fixed sentence.
Traces: backup-restore-job-refusals#task#1.

=== Cause 5 of 7: StartRestoreJob's second refusal omits its Code ===
AWS: Smithy: the StartRestoreJob 400 refusal structures declare a Code member. Recorded behavior: this refusal carries Code 'ERROR_3806' alongside the message the emulator already matches, so Code is the only divergence.
Emulator: $.Code missing (expected 'ERROR_3806', got None) — the handler's refusal is built without the service's error identifier.
Why: AWS Backup clients key retry and reporting decisions off these ERROR_* codes; with Code absent, two refusals that share a message class cannot be distinguished programmatically.
Fix: the refusal construction in the StartRestoreJob handler / the crate's error serializer, which must populate Code for every refusal it emits.
Traces: backup-restore-job-refusals#task#2.

=== Cause 6 of 7: StartScanJob's invalid-enum refusal is missing AWS's validation-error prefix, echoes the submitted value, orders the enum set differently and uses the wrong error type ===
AWS: Smithy: StartScanJob (PUT /scan/job, success 201) requires ScanMode targeting enum ScanMode with values [FULL_SCAN, INCREMENTAL_SCAN]. Recorded behavior: an out-of-set ScanMode is refused 400 with a lowercase `message` field reading "1 validation error detected: Value at 'scanMode' failed to satisfy constraint: Member must satisfy enum value set: [INCREMENTAL_SCAN, FULL_SCAN]" — the fixed text is "1 validation error detected: Value at '", "' failed to satisfy constraint: Member must satisfy enum value set: [INCREMENTAL_SCAN, FULL_SCAN]"; the only variable part is the member path (`scanMode`), and the submitted value is NOT echoed. The error type is the framework validation shape, not InvalidParameterValueException.
Emulator: answers 400 with `message` = "Value 'PARTIAL_SCAN' at 'scanMode' failed to satisfy constraint: Member must satisfy enum value set: [FULL_SCAN, INCREMENTAL_SCAN]" and x-amzn-errortype InvalidParameterValueException. Four divergences in one: the "1 validation error detected: " prefix is missing, the offending value is interpolated where AWS names none, the enum set is rendered in model order instead of AWS's rendering order, and the error type is a service refusal rather than the validation shape.
Why: this is the highest-volume cluster (8 steps): any client or test asserting on AWS's validation wording or matching the validation error type fails on a correctly-refused request, and a message that quotes the rejected value is a different template from AWS's.
Fix: the modeled-constraint validation rendering for this operation (the shared pre-handler validation walk that formats enum violations) and the error type it is reported under.
Traces: backup-scan-job-start-refusals#task#3 (cluster also covers backup-start-scan-job-required-member-validation#task#1 through #task#7).

=== Cause 7 of 7: StartReportJob's invalid report-plan-name refusal carries no message at all ===
AWS: Smithy: StartReportJob is POST /audit/report-jobs/{ReportPlanName} with ReportPlanName @length 1..256 and @pattern ^[a-zA-Z][_a-zA-Z0-9]*$, and models InvalidParameterValueException/MissingParameterValueException/ResourceNotFoundException (400). Recorded behavior: a name violating that grammar is refused 400 with the message 'Invalid report plan name <name>. Report plan names must be between 1 and 256 characters, start with a letter (A-Z and a-z), and consist only of letters, numbers (0-9), and underscores (_). Type a valid report plan name.' — everything after the name is fixed text, and the single variable part is the report plan name taken verbatim from the request path.
Emulator: answers 400 with $.Message missing entirely (got None): the status is right, the body says nothing.
Why: the caller learns only that the call failed, not that the plan NAME is malformed nor what the grammar is — and because the same empty 400 is used for other refusals of this operation, a not-found plan and an invalid name are indistinguishable.
Fix: the ReportPlanName validation in the StartReportJob handler — it must emit AWS's sentence with the submitted name interpolated.
Traces: backup-report-job-plan-name-grammar-refusals#task#1 (cluster also covers backup-report-job-plan-name-grammar-refusals#task#2, #task#3 and backup-report-job-start-refusals#task#2).

## Evidence
Replay of trace set simparity-wire-aws-v1 (103 fixtures, 640 steps, 538 passed, 98 mismatched, score 84.1%, bootstrap_signal false, emulator_up true). Seven divergence clusters, verdict=mismatch, service=backup, none carrying stub_path or owned_by:

Cause 1 — backup-start-backup-job-omitted-role-refused-as-access-denied#task#4, StartBackupJob, expected 403 / actual 400:
  status: expected 403, got 400
  $.Message: missing (expected 'Insufficient privileges to perform this action.', got None)
  $headers.x-amzn-errortype: shape_mismatch (got 'MissingParameterValueException')

Cause 2 — backup-start-backup-job-omitted-role-refused-as-access-denied#task#5, StartBackupJob, expected 400 / actual 200:
  status: expected 400, got 200
  $.Code: missing (expected 'ERROR_3020', got None)
  $.Message: missing (expected 'IAM Role does not have sufficient permissions to execute the backup', got None)
  $headers.x-amzn-errortype: missing (got None — no error was returned; the call succeeded)

Cause 3 — backup-start-backup-job-omitted-role-refused-as-access-denied#teardown#6, DeleteBackupVault, expected 200 / actual 400:
  status: expected 200, got 400

Cause 4 — backup-restore-job-refusals#task#1, StartRestoreJob, expected 400 / actual 400:
  $.Code: missing (expected 'ERROR_4003', got None)
  $.Message: value_mismatch (expected 'IAM Role is null', got 'Cannot find recovery point')
  $headers.x-amzn-errortype: shape_mismatch (got 'InvalidParameterValueException')

Cause 5 — backup-restore-job-refusals#task#2, StartRestoreJob, expected 400 / actual 400:
  $.Code: missing (expected 'ERROR_3806', got None)

Cause 6 — backup-scan-job-start-refusals#task#3, StartScanJob, expected 400 / actual 400, affected_count 8:
  $.message: value_mismatch (expected "1 validation error detected: Value at 'scanMode' failed to satisfy constraint: Member must satisfy enum value set: [INCREMENTAL_SCAN, FULL_SCAN]", got "Value 'PARTIAL_SCAN' at 'scanMode' failed to satisfy constraint: Member must satisfy enum value set: [FULL_SCAN, INCREMENTAL_SCAN]")
  $headers.x-amzn-errortype: shape_mismatch (got 'InvalidParameterValueException')
  Other members: backup-start-scan-job-required-member-validation#task#1..#task#7.

Cause 7 — backup-report-job-plan-name-grammar-refusals#task#1, StartReportJob, expected 400 / actual 400, affected_count 4:
  $.Message: missing (expected 'Invalid report plan name <name>. Report plan names must be between 1 and 256 characters, start with a letter (A-Z and a-z), and consist only of letters, numbers (0-9), and underscores (_). Type a valid report plan name.', got None) — the recorded run's <name> was '9sim2real_probe_plan', which is the request's value, not part of the template.
  Other members: backup-report-job-plan-name-grammar-refusals#task#2, #task#3, backup-report-job-start-refusals#task#2.

Model evidence (services/service-com-amazonaws-backup-cryocontrollerusermanager/smithy-model/com-amazonaws-backup-cryocontrollerusermanager.json): StartBackupJob PUT /backup-jobs (required BackupVaultName, ResourceArn, IamRoleArn; AccessDeniedException not modeled); DeleteBackupVault DELETE /backup-vaults/{BackupVaultName}, output Unit, code 200; StartRestoreJob PUT /restore-jobs; StartScanJob PUT /scan/job code 201, ScanMode enum values [FULL_SCAN, INCREMENTAL_SCAN]; StartReportJob POST /audit/report-jobs/{ReportPlanName}, ReportPlanName length 1..256 pattern ^[a-zA-Z][_a-zA-Z0-9]*$. Every modeled 400 error structure (InvalidParameterValueException, MissingParameterValueException, ResourceNotFoundException, InvalidRequestException) declares members Code, Message, Type, Context.

Note on values: role names, vault names, plan names and the rejected 'PARTIAL_SCAN' above are one run's request values. The requirements are the statuses, the error codes/types, and the fixed message text with request-derived variable parts as annotated per cause.

## Steps to Reproduce
Replay the named fixture steps against the emulator (trace set simparity-wire-aws-v1).

Cause 1: backup-start-backup-job-omitted-role-refused-as-access-denied, step task#4 — StartBackupJob against an existing vault with IamRoleArn omitted. Expect HTTP 403 with Message 'Insufficient privileges to perform this action.'; the emulator returns 400 MissingParameterValueException with no Message.
Cause 2: same fixture, step task#5 — StartBackupJob with a role that lacks backup permissions. Expect HTTP 400 with Code 'ERROR_3020' and Message 'IAM Role does not have sufficient permissions to execute the backup' and NO job created; the emulator returns 200 with a job.
Cause 3: same fixture, step teardown#6 — DeleteBackupVault on the probe vault. Expect HTTP 200 with an empty body; the emulator returns 400 (re-check after cause 2 is fixed, since the spurious job may be what blocks the delete).
Cause 4: backup-restore-job-refusals, step task#1 — StartRestoreJob with no IamRoleArn. Expect 400 Code 'ERROR_4003', Message 'IAM Role is null'; the emulator answers InvalidParameterValueException 'Cannot find recovery point' with no Code.
Cause 5: backup-restore-job-refusals, step task#2 — expect the refusal to carry Code 'ERROR_3806'; the emulator omits Code.
Cause 6: backup-scan-job-start-refusals, step task#3 (and backup-start-scan-job-required-member-validation task#1..#7) — StartScanJob with ScanMode outside [FULL_SCAN, INCREMENTAL_SCAN]. Expect 400, message "1 validation error detected: Value at 'scanMode' failed to satisfy constraint: Member must satisfy enum value set: [INCREMENTAL_SCAN, FULL_SCAN]" under the validation error type; the emulator prefixes nothing, echoes the submitted value, reverses the enum set and reports InvalidParameterValueException.
Cause 7: backup-report-job-plan-name-grammar-refusals, step task#1 — StartReportJob with a report plan name that breaks ^[a-zA-Z][_a-zA-Z0-9]*$ (e.g. a leading digit). Expect 400 with 'Invalid report plan name <that name>. Report plan names must be between 1 and 256 characters, start with a letter (A-Z and a-z), and consist only of letters, numbers (0-9), and underscores (_). Type a valid report plan name.'; the emulator returns 400 with no Message.
