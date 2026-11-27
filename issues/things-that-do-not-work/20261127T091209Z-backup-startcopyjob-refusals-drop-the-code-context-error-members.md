Issue-Source: trace-sweep-divergence
Issue-Trace: backup-copy-job-missing-parameters
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: backup-copy-job-missing-parameters backup-start-copy-job-parameter-validation backup-copy-job-start-refusals
Issue-Shape-Id: com.amazonaws.backup#CryoControllerUserManager
Issue-Key: backup|StartCopyJob|4xx->4xx|$.Code
Issue-Key: backup|StartCopyJob|4xx->4xx|$.Message
Issue-Key: backup|StartCopyJob|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: backup|StartCopyJob|4xx->4xx|$.Context
Issue-Commit: da5b5c82f86ff32a5f1d8fa609cf9d0b9820cd03

# StartCopyJob refusals drop the Code/Context error members and mis-order parameter vs resource validation (5 causes)

> _Note: this issue was identified against the code base at commit `da5b5c82f86ff32a5f1d8fa609cf9d0b9820cd03`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartCopyJob

## Description
Five distinct root causes behind the StartCopyJob refusal clusters in the wire corpus, all in the AWS Backup crate (services/service-com-amazonaws-backup-cryocontrollerusermanager), bundled as one work item for the `backup` service. Each must be closed separately. None of these clusters is marked ALREADY FILED (stub_path) or OWNED (owned_by) in the replay report, and no open backup issue names StartCopyJob (the nearest ones — 20261029T175130Z-backup-aws-backup-vault-lock-and-tiering-refusals-use.md, 20261126T005208Z-backup-listbackupplanversions-returns-a-plan-s-versions-oldest-first.md — are about vault-lock/tiering refusals and StartBackupJob authorization, and about ListBackupPlanVersions ordering).

The shared fault: AWS Backup's refusal structures carry four members — Smithy: InvalidParameterValueException / MissingParameterValueException / ResourceNotFoundException / InvalidRequestException each have members {Code, Message, Type, Context}, all HTTP 400 for this REST-JSON service. The emulator serializes at most `Message`, never populates `Code`/`Context` with the service's own ERROR_* identifiers, and in two cases picks the wrong refusal class because it resolves the recovery point before validating parameters.

=== Cause 1 of 5: a null destination/source Backup Vault ARN is refused with an empty error body — no Code, no Message ===
AWS: Smithy: StartCopyJob input requires RecoveryPointArn, SourceBackupVaultName, DestinationBackupVaultArn and IamRoleArn; the modeled missing-value refusal is MissingParameterValueException (HTTP 400) whose members include Code and Message. Recorded behavior: omitting the vault ARN is refused 400 with Code 'ERROR_41005' and Message 'Backup Vault ARN is null' — the whole message is fixed text, nothing from the request is interpolated, and Code is the service's own stable identifier.
Emulator: answers HTTP 400 (the status class is right) but with BOTH members absent: $.Code is missing (expected 'ERROR_41005', got None) and $.Message is missing (expected 'Backup Vault ARN is null', got None). The refusal body carries no identifying content at all.
Why: a client cannot tell WHICH required parameter it omitted, and anything branching on the service's Code (ERROR_41005) or surfacing the message to an operator gets nothing; an empty-bodied 400 is indistinguishable from any other refusal.
Fix: the refusal serializer / missing-parameter validation of the StartCopyJob handler in the AWS Backup crate — it must populate Code and Message (and Context where AWS does) on the MissingParameterValueException it already returns.
Traces: backup-copy-job-missing-parameters#task#3 (representative; cluster also covers backup-copy-job-missing-parameters#task#4, #task#5 and backup-start-copy-job-parameter-validation#task#5).

=== Cause 2 of 5: a syntactically invalid IamRoleArn is classified as a MISSING parameter and carries no message ===
AWS: Smithy: IamRoleArn targets IAMRoleArn and is required; an ill-formed (as opposed to absent) value is the invalid-value class, not the missing-value class — Smithy models both InvalidParameterValueException ('Indicates that something is wrong with a parameter's value') and MissingParameterValueException ('Indicates that a required parameter is missing') on StartCopyJob. Recorded behavior: the refusal is 400 with Message 'Provided IamRoleArn is invalid.' — entirely fixed text (the offending ARN is NOT echoed) — carried by an error shape other than MissingParameterValueException.
Emulator: answers 400 with x-amzn-errortype MissingParameterValueException (shape_mismatch against the recorded shape) and $.Message missing (expected 'Provided IamRoleArn is invalid.', got None).
Why: an SDK matching on error type takes the "you forgot a parameter" branch for a parameter that WAS supplied but malformed, so the caller is told to add a value it already sent, and there is no message explaining the real problem.
Fix: the IamRoleArn validation in the StartCopyJob handler — a present-but-malformed role ARN must be refused in the invalid-value class with AWS's fixed sentence, distinct from the absent-value path of cause 1.
Traces: backup-copy-job-missing-parameters#task#6.

=== Cause 3 of 5: the not-found refusal for a recovery point omits Code and the Context member that names the offending ARN ===
AWS: Smithy: StartCopyJob models ResourceNotFoundException (HTTP 400) with members {Code, Message, Type, Context}. Recorded behavior: a RecoveryPointArn that does not exist in the source vault is refused 400 with Code 'ERROR_2603' and Context set to the recovery-point ARN taken from the request — i.e. Context's value is a request value, while Code is fixed text; the message matched the emulator, so only these two members diverge.
Emulator: $.Code missing (expected 'ERROR_2603', got None) and $.Context missing (expected the request's recovery-point ARN, got None).
Why: AWS Backup clients and the console use Code to key refusal handling and Context to show the operator exactly which resource was not found; with both absent a caller cannot programmatically attribute the failure to the ARN it passed.
Fix: the not-found refusal construction in the StartCopyJob handler / the crate's error serializer — Code must carry the service's ERROR_2603 identifier and Context the ARN from the request.
Traces: backup-copy-job-start-refusals#task#3.

=== Cause 4 of 5: a role that AWS Backup cannot assume is reported as a missing recovery point ===
AWS: Smithy: StartCopyJob models InvalidParameterValueException and ResourceNotFoundException (both 400) with {Code, Message, Type, Context}. Recorded behavior: when IamRoleArn names a role AWS Backup cannot assume, the refusal is 400 with Code 'ERROR_3017', Message 'IAM Role cannot be assumed by AWS Backup' (fixed text, no interpolation) and Context set to the role ARN from the request — the role is evaluated before the recovery point is resolved.
Emulator: answers 400 ResourceNotFoundException with Message 'Cannot find recovery point', and no Code and no Context. Hypothesis: the handler resolves the recovery point first and has no role-assumability check at all, so the recovery-point lookup refuses before the role is ever considered.
Why: the caller is told to fix the recovery point when the actual problem is the role's trust policy; a client that retries after re-creating a recovery point loops forever, and error-type matching on the role-failure class never fires.
Fix: the validation ordering and the role check in the StartCopyJob handler — the role's assumability must be decided before the recovery-point lookup, and refused with Code ERROR_3017, AWS's fixed sentence and Context = the request's role ARN.
Traces: backup-copy-job-start-refusals#task#4 (cluster also covers backup-start-copy-job-parameter-validation#task#6 and #task#7).

=== Cause 5 of 5: a malformed RecoveryPointArn is refused with a generic invented Code/Message/Context template instead of the service's own ===
AWS: Smithy: RecoveryPointArn targets ARN with no @pattern, so the shape is refused by the service, not by a modeled constraint; the modeled refusal is InvalidParameterValueException (400) with {Code, Message, Type, Context}. Recorded behavior: an ill-formed recovery-point ARN is refused 400 with Code 'ERROR_3603', Message 'Invalid recovery point ARN' — fixed text that names neither the member nor the submitted value — and Context left null.
Emulator: answers Code 'INVALID_PARAMETER_VALUE', Message 'Parameter RecoveryPointArn is invalid.' and Context 'RecoveryPointArn' — a generic per-member template (the crate's rejected_arn-style rendering) rather than this refusal's own wording, and it fills Context with a member NAME where AWS leaves Context unset (AWS uses Context for resource ARNs, see cause 3).
Why: the Code is not one AWS ever emits, so Code-keyed handling breaks; the message differs in wording and shape (AWS names no member, the emulator does); and Context is populated with a member name, which misleads any client reading Context as a resource identifier.
Fix: the ARN-syntax validation of the StartCopyJob handler — RecoveryPointArn's refusal needs the service's own Code ERROR_3603 and fixed sentence, with Context unset, instead of the shared generic parameter template.
Traces: backup-copy-job-start-refusals#task#5 (cluster also covers backup-start-copy-job-parameter-validation#task#2 and #task#4).

## Evidence
Replay of trace set simparity-wire-aws-v1 (103 fixtures, 640 steps, 538 passed, score 84.1%, bootstrap_signal false, emulator_up true). Five StartCopyJob divergence clusters, all verdict=mismatch, service=backup, no stub_path and no owned_by:

Cause 1 — backup-copy-job-missing-parameters#task#3, StartCopyJob, expected 400 / actual 400, affected_count 4:
  $.Code: missing (expected 'ERROR_41005', got None)
  $.Message: missing (expected 'Backup Vault ARN is null', got None)
  Other members: backup-copy-job-missing-parameters#task#4, #task#5, backup-start-copy-job-parameter-validation#task#5.

Cause 2 — backup-copy-job-missing-parameters#task#6, StartCopyJob, expected 400 / actual 400:
  $.Message: missing (expected 'Provided IamRoleArn is invalid.', got None)
  $headers.x-amzn-errortype: shape_mismatch (recorded shape was NOT asserted-equal to 'MissingParameterValueException', which is what the emulator returned)

Cause 3 — backup-copy-job-start-refusals#task#3, StartCopyJob, expected 400 / actual 400:
  $.Code: missing (expected 'ERROR_2603', got None)
  $.Context: missing (expected the request's recovery-point ARN, e.g. 'arn:aws:backup:<region>:<account>:recovery-point:<id>', got None)

Cause 4 — backup-copy-job-start-refusals#task#4, StartCopyJob, expected 400 / actual 400, affected_count 3:
  $.Code: missing (expected 'ERROR_3017', got None)
  $.Context: missing (expected the request's IAM role ARN, e.g. 'arn:aws:iam::<account>:role/<role>', got None)
  $.Message: value_mismatch (expected 'IAM Role cannot be assumed by AWS Backup', got 'Cannot find recovery point')
  $headers.x-amzn-errortype: shape_mismatch (got 'ResourceNotFoundException')
  Other members: backup-start-copy-job-parameter-validation#task#6, #task#7.

Cause 5 — backup-copy-job-start-refusals#task#5, StartCopyJob, expected 400 / actual 400, affected_count 3:
  $.Code: value_mismatch (expected 'ERROR_3603', got 'INVALID_PARAMETER_VALUE')
  $.Context: type_mismatch (expected None, got 'RecoveryPointArn')
  $.Message: value_mismatch (expected 'Invalid recovery point ARN', got 'Parameter RecoveryPointArn is invalid.')
  Other members: backup-start-copy-job-parameter-validation#task#2, #task#4.

Model evidence (services/service-com-amazonaws-backup-cryocontrollerusermanager/smithy-model/com-amazonaws-backup-cryocontrollerusermanager.json): StartCopyJob PUT /copy-jobs, required RecoveryPointArn/SourceBackupVaultName/DestinationBackupVaultArn/IamRoleArn, errors InvalidParameterValueException, InvalidRequestException, LimitExceededException, MissingParameterValueException, ResourceNotFoundException (all client, HTTP 400) and ServiceUnavailableException (500); every one of those error structures declares members Code, Message, Type, Context.

Note on values: the recorded ARNs, role names and account ids above are one run's values. The requirement is the template — fixed sentences exactly as quoted, Code exactly as quoted, and Context carrying the ARN from the request (or unset, for cause 5).

## Steps to Reproduce
Replay the fixtures named under each cause against the emulator (trace set simparity-wire-aws-v1); each is a PUT /copy-jobs call.

Cause 1: replay backup-copy-job-missing-parameters step task#3 — StartCopyJob with the vault ARN omitted. Expect 400 with Code 'ERROR_41005' and Message 'Backup Vault ARN is null'; the emulator returns 400 with neither member.
Cause 2: replay backup-copy-job-missing-parameters step task#6 — StartCopyJob with a malformed IamRoleArn. Expect 400, Message 'Provided IamRoleArn is invalid.', error type NOT MissingParameterValueException; the emulator returns MissingParameterValueException with no Message.
Cause 3: replay backup-copy-job-start-refusals step task#3 — StartCopyJob naming a recovery point that does not exist in the source vault. Expect 400 with Code 'ERROR_2603' and Context = the RecoveryPointArn sent; the emulator omits both.
Cause 4: replay backup-copy-job-start-refusals step task#4 — StartCopyJob with an IamRoleArn AWS Backup cannot assume. Expect 400 Code 'ERROR_3017', Message 'IAM Role cannot be assumed by AWS Backup', Context = the role ARN sent; the emulator answers ResourceNotFoundException 'Cannot find recovery point'.
Cause 5: replay backup-copy-job-start-refusals step task#5 — StartCopyJob with a malformed RecoveryPointArn. Expect 400 Code 'ERROR_3603', Message 'Invalid recovery point ARN', Context unset; the emulator answers Code 'INVALID_PARAMETER_VALUE', Message 'Parameter RecoveryPointArn is invalid.', Context 'RecoveryPointArn'.
