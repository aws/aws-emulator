Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: backup.start-backup-job-nonexistent-resource-job-fails backup.start-restore-job-invalid-metadata-rejected
Issue-Shape-Id: com.amazonaws.backup#CryoControllerUserManager
Issue-Key: backup.start-backup-job-nonexistent-resource-job-fails|run#10|AssertionError|assert job["State"] == "FAILED", (
Issue-Key: backup.start-restore-job-invalid-metadata-rejected|run#19|AssertionError|assert code == "InvalidParameterValueException", f"expected InvalidParameterValueException, got {code}"
Issue-Commit: 110c1f7d17acc861e9ef51d16e30161c9b1319a9

# AWS Backup: 2 net-new causes — a backup job against a nonexistent resource settles COMPLETED instead of FAILED/RESOURCE_NOT_FOUND, and StartRestoreJob answers 501 NotImplemented instead of validating restore Metadata

> _Note: this issue was identified against the code base at commit `110c1f7d17acc861e9ef51d16e30161c9b1319a9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartBackupJob

## Description
Two distinct root causes in the AWS Backup emulator's job plane, both grounded in failing behavioral checks of the `backup` service. Each cause is numbered and carries its own failing check ids.

Cause 1 of 2: StartBackupJob accepts a well-formed ARN naming a resource that does not exist and the resulting job settles COMPLETED, with no failure StatusMessage and no MessageCategory.
  AWS: Smithy (services/service-com-amazonaws-backup-cryocontrollerusermanager/smithy-model/com-amazonaws-backup-cryocontrollerusermanager.json): StartBackupJob input requires BackupVaultName/ResourceArn/IamRoleArn and returns BackupJobId + RecoveryPointArn; the ARN's existence is NOT validated synchronously, so the call is accepted (200). DescribeBackupJob output models `State` (enum BackupJobState), `StatusMessage` (string) and `MessageCategory` (string). Real AWS runs the job asynchronously and it reaches the terminal state FAILED when the ResourceArn names no existing resource, with StatusMessage of the fixed form "Requested resource not found: ResourceArn: <resource-arn> not found" — fixed text "Requested resource not found: ResourceArn: " and " not found", with the ResourceArn taken verbatim from the request — and MessageCategory "RESOURCE_NOT_FOUND". Such a FAILED job mints no recovery point, so DescribeRecoveryPoint on the RecoveryPointArn the job reports is refused 400 ResourceNotFoundException with message "Cannot find recovery point" (fixed text, no variable part). Recorded behavior confirms the same job shape against a table that really exists ends COMPLETED.
  Emulator: the job against the nonexistent DynamoDB table reaches terminal state 'COMPLETED' (assertion: "expected the job against a nonexistent resource to end FAILED, got 'COMPLETED'"). Every job settles COMPLETED regardless of whether the ResourceArn resolves, so the FAILED branch — its StatusMessage, its MessageCategory 'RESOURCE_NOT_FOUND', and the absence of a minted recovery point — is unreachable.
  Why: a client that polls DescribeBackupJob to learn whether a backup actually captured anything is told the backup succeeded for a resource that does not exist. Backup orchestration, compliance reporting and alerting built on State/MessageCategory silently treat a non-backup as a protected recovery point, and code that reads the job's RecoveryPointArn gets an ARN the emulator also resolves, so the error path is never exercised.
  Fix: the StartBackupJob handler's job-progression/state machine and its backing job state — the resource-resolution step that decides a job's terminal state must resolve ResourceArn against the owning service's state and, when it does not resolve, settle the job FAILED with the StatusMessage and MessageCategory above and mint no recovery point (so DescribeRecoveryPoint on the job's RecoveryPointArn is refused ResourceNotFoundException "Cannot find recovery point"). Note: open issue 20261024T031359Z-backup-events-aws-backup-job-lifecycle-and-eventing-diverge-jobs.md describes a related 'jobs always settle COMPLETED' symptom under the backup__events service; this check was not marked owned by it and is filed here under `backup`.
  Traces: backup.start-backup-job-nonexistent-resource-job-fails.

Cause 2 of 2: StartRestoreJob answers 501 NotImplemented, so DynamoDB restore-metadata validation never happens.
  AWS: Smithy: StartRestoreJob (PUT /restore-jobs, success 200) requires RecoveryPointArn and Metadata and models InvalidParameterValueException (400) among its errors. For a DynamoDB recovery point, Metadata that omits the required TargetTableName key is refused 400 InvalidParameterValueException. Recorded behavior: the message has the fixed text "Invalid restore metadata. Target table name must be provided with restore metadata. Restore metadata validation failed for DynamoDB with recoveryPoint " followed by the recovery point ARN taken from the request, and a trailing "." — i.e. "Invalid restore metadata. Target table name must be provided with restore metadata. Restore metadata validation failed for DynamoDB with recoveryPoint <recovery-point-arn>." The refused call creates no restore job (ListRestoreJobs lists none for that recovery point), and the same call with Metadata {"TargetTableName": <name>} is accepted and the restore job runs to COMPLETED.
  Emulator: the call is refused with error code 'NotImplemented' (assertion: "expected InvalidParameterValueException, got NotImplemented") — "operation StartRestoreJob is scaffolded but not implemented in this emulator" — for both the malformed-Metadata call and the corrected one, so neither the parameter-value refusal nor the restore itself is reachable.
  Why: a client cannot distinguish a malformed restore request from an unavailable service: it sees a server-side 'not implemented' class instead of the 400 client-error class it must catch, so retry/validation logic misfires and no restore path (restore job creation, DescribeRestoreJob, ListRestoreJobs, the restored table) can be exercised at all.
  Fix: implement the StartRestoreJob handler (services/service-com-amazonaws-backup-cryocontrollerusermanager/src/ops/start_restore_job.rs) together with its Metadata validation for the recovery point's ResourceType — reject DynamoDB Metadata lacking TargetTableName with 400 InvalidParameterValueException and the message above, persisting no restore job; on acceptance mint a restore job that progresses to COMPLETED and is listed by ListRestoreJobs for that recovery point. Related open issues: 20261029T175130Z-backup-startrestorejob-stub.md and 20261108T091930Z-backup-events-backup-startrestorejob-answers-notimplemented-501-instead-of-synchronously.md; this check was marked neither ALREADY FILED nor OWNED by either, so it is filed here.
  Traces: backup.start-restore-job-invalid-metadata-rejected.

## Evidence
Cause 1 — backup.start-backup-job-nonexistent-resource-job-fails [mismatch]:
  AssertionError: expected the job against a nonexistent resource to end FAILED, got 'COMPLETED'
  Expected (AWS): DescribeBackupJob -> State 'FAILED', StatusMessage "Requested resource not found: ResourceArn: arn:aws:dynamodb:<region>:<account>:table/<never-created-table> not found", MessageCategory 'RESOURCE_NOT_FOUND'; DescribeRecoveryPoint on the job's RecoveryPointArn -> 400 ResourceNotFoundException, message "Cannot find recovery point". The identical call against a real table -> State 'COMPLETED'.
  Actual (emulator): DescribeBackupJob -> State 'COMPLETED' for the nonexistent resource; the FAILED branch, its StatusMessage/MessageCategory and the no-recovery-point consequence are never produced.

Cause 2 — backup.start-restore-job-invalid-metadata-rejected [mismatch]:
  AssertionError: expected InvalidParameterValueException, got NotImplemented
  Expected (AWS): start_restore_job(RecoveryPointArn=<ddb recovery point>, Metadata={"NotARealKey": "x"}, IamRoleArn=<role>) -> ClientError 400, Error.Code 'InvalidParameterValueException', Error.Message "Invalid restore metadata. Target table name must be provided with restore metadata. Restore metadata validation failed for DynamoDB with recoveryPoint <recovery-point-arn>."; no restore job created; the same call with Metadata={"TargetTableName": <name>} accepted, job -> COMPLETED, ListRestoreJobs lists exactly that job for the recovery point.
  Actual (emulator): ClientError Error.Code 'NotImplemented', message "operation StartRestoreJob is scaffolded but not implemented in this emulator".

## Steps to Reproduce
Cause 1 (backup.start-backup-job-nonexistent-resource-job-fails):
1. CreateBackupVault with a fresh name; create an IAM role trusted by backup.amazonaws.com and attach AWSBackupServiceRolePolicyForBackup.
2. StartBackupJob with BackupVaultName=<vault>, IamRoleArn=<role>, ResourceArn="arn:aws:dynamodb:<region>:<account>:table/<a-table-that-was-never-created>" — the call is accepted.
3. Poll DescribeBackupJob(BackupJobId) until terminal. Observe State='COMPLETED'; AWS reports State='FAILED', StatusMessage="Requested resource not found: ResourceArn: <that arn> not found", MessageCategory='RESOURCE_NOT_FOUND'.
4. DescribeRecoveryPoint(BackupVaultName=<vault>, RecoveryPointArn=<job's RecoveryPointArn>) — AWS refuses 400 ResourceNotFoundException "Cannot find recovery point".

Cause 2 (backup.start-restore-job-invalid-metadata-rejected):
1. Create a DynamoDB table, enable PITR (UpdateContinuousBackups), create a backup vault and a backup service role.
2. StartBackupJob for the table's ARN and wait for the job to complete; take its RecoveryPointArn.
3. StartRestoreJob(RecoveryPointArn=<rp>, Metadata={"NotARealKey": "x"}, IamRoleArn=<role>). Observe Error.Code 'NotImplemented'; AWS answers 400 InvalidParameterValueException with the restore-metadata message above and creates no restore job.
4. StartRestoreJob(RecoveryPointArn=<rp>, Metadata={"TargetTableName": <new name>}, IamRoleArn=<role>) — AWS accepts and the restore job reaches COMPLETED, creating the target table.
