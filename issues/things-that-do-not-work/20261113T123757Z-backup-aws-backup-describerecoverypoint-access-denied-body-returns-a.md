Issue-Source: trace-sweep-divergence
Issue-Trace: backup-recovery-point-describe-and-metadata-not-found
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: backup-recovery-point-describe-and-metadata-not-found backup-recovery-point-error-paths-on-live-vault
Issue-Shape-Id: com.amazonaws.backup#CryoControllerUserManager
Issue-Key: backup|DescribeRecoveryPoint|4xx->4xx|$.Message
Issue-Commit: 4370547da342f3cac1e3d2a524b52e751b2d7b18

# AWS Backup DescribeRecoveryPoint access-denied body returns a generic message instead of the AWS authorization sentence

> _Note: this issue was identified against the code base at commit `4370547da342f3cac1e3d2a524b52e751b2d7b18`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeRecoveryPoint

## Description
Cause 1 of 1: DescribeRecoveryPoint on a recovery-point ARN owned by an account other than the caller's is now correctly refused with HTTP 403 (the status and error class agree with the recording), but the refusal body's $.Message is a fixed generic emulator sentence instead of AWS's authorization message.

AWS: Smithy (com.amazonaws.backup#CryoControllerUserManager, services/service-com-amazonaws-backup-cryocontrollerusermanager/smithy-model/com-amazonaws-backup-cryocontrollerusermanager.json): DescribeRecoveryPoint is GET /backup-vaults/{BackupVaultName}/recovery-points/{RecoveryPointArn} with BackupVaultName (httpLabel, required, @pattern ^[a-zA-Z0-9\-\_]{2,50}$), RecoveryPointArn (httpLabel, required, ARN) and the optional BackupVaultAccountId (httpQuery backupVaultAccountId, @pattern ^[0-9]{12}$); its modeled errors are InvalidParameterValueException, MissingParameterValueException and ResourceNotFoundException (all client, HTTP 400) plus ServiceUnavailableException (server, 500). AccessDeniedException is NOT in that modeled set, so for the body of this refusal this is recorded behavior only: the recording answers HTTP 403 with the standard AWS authorization sentence, whose template is "User: <callerPrincipalArn> is not authorized to perform: backup:DescribeRecoveryPoint on resource: <recoveryPointArn> because no resource-based policy allows the backup:DescribeRecoveryPoint action". The FIXED text of that message is "User: ", " is not authorized to perform: ", " on resource: ", " because no resource-based policy allows the " and " action". The VARIABLE parts are the caller's principal ARN (from the signing credentials), the action name (the service prefix plus the operation name, i.e. backup:DescribeRecoveryPoint, which IS required) and the RecoveryPointArn taken verbatim from the request path. The principal ARN, the foreign account id and the recovery-point uuid seen in any one recording are only that run's fill-ins for the template, never requirements.

Emulator: the authorization refusal itself is correct now (HTTP 403, same status and error class as the recording), but the body carries the fixed literal $.Message 'Insufficient privileges to perform this action.' with no principal, no action name and no resource ARN. The same single sentence is served for every unauthorized DescribeRecoveryPoint, whatever the caller, the action or the target recovery point.

Why: a client, auditor or operator that parses AWS's authorization sentence to learn WHICH principal lacked WHICH action on WHICH resource gets nothing actionable: it cannot name the missing IAM permission (backup:DescribeRecoveryPoint), cannot identify the recovery point the call was refused on, and cannot distinguish a resource-policy refusal from any other privilege failure. Permission-advisory tooling, log scrapers and tests that assert on the AWS wording fail against the emulator even though the status and error code now match.

Fix: belongs in the shared authorization/refusal rendering path the backup recovery-point handlers call (service crate services/service-com-amazonaws-backup-cryocontrollerusermanager — DescribeRecoveryPoint's handler and the AccessDeniedException serialization it delegates to): render the message from the template above, substituting the caller principal ARN from the request's credentials, the action name derived from the operation (backup:DescribeRecoveryPoint) and the RecoveryPointArn from the request path, rather than emitting a fixed generic sentence.

Related issues, none of which cover this: 20261030T221324Z-backup-aws-backup-describerecoverypoint-performs-no-authorization-a-cross.md is RESOLVED and covered the STATUS/error-class divergence (400 ResourceNotFoundException instead of 403 AccessDeniedException) that is now fixed — a resolved issue never covers a live failure, and the replay reports this cluster with no owner. 20261112T011103Z-backup-aws-backup-deleterecoverypoint-access-denied-body-returns-a.md is the open sibling for the SAME defect on DeleteRecoveryPoint and owns only that operation's cluster; the fix is the shared rendering path both handlers use. 20261030T020405Z-backup-aws-backup-framework-and-recovery-point-reads-are.md and 20261030T020405Z-backup-describerecoverypoint-stub.md are resolved 501-stub issues and are unrelated to this body divergence.

Traces: backup-recovery-point-describe-and-metadata-not-found#task#3, backup-recovery-point-error-paths-on-live-vault#task#4 (same cluster, affected_count 2).

## Evidence
Divergence cluster service=backup|DescribeRecoveryPoint|status=403->403|keys=$.Message (verdict: mismatch, affected_count 2, stub_path empty, owned_by empty — net-new).

Expected (recording): HTTP 403; $.Message = 'User: <callerPrincipalArn> is not authorized to perform: backup:DescribeRecoveryPoint on resource: <recoveryPointArn> because no resource-based policy allows the backup:DescribeRecoveryPoint action'.
Actual (emulator): HTTP 403; $.Message = 'Insufficient privileges to perform this action.'

Replay detail, verbatim: "$.Message: value_mismatch (expected 'User: arn:aws:iam::000000000000:user/emulator is not authorized to perform: backup:DescribeRecoveryPoint on resource: arn:aws:backup:us-west-2:123456789012:recovery-point:fb698c18-a985-4704-b4fb-0c14fa8cfaa3 because no resource-based policy allows the backup:DescribeRecoveryPoint action', got 'Insufficient privileges to perform this action.')".

In that recorded message the FIXED text is "User: ... is not authorized to perform: ... on resource: ... because no resource-based policy allows the ... action". The VARIABLE parts are the principal ARN (arn:aws:iam::000000000000:user/emulator — taken from the caller's credentials), the foreign account id (123456789012) and the recovery-point uuid (fb698c18-a985-4704-b4fb-0c14fa8cfaa3) inside the RecoveryPointArn — taken verbatim from the request path. Those values are this run's fill-ins, not literal requirements. The action name 'backup:DescribeRecoveryPoint' is derived from the operation and IS required.

Affected steps: backup-recovery-point-describe-and-metadata-not-found#task#3, backup-recovery-point-error-paths-on-live-vault#task#4.

## Steps to Reproduce
1. Replay fixture backup-recovery-point-describe-and-metadata-not-found, step task#3: call backup.DescribeRecoveryPoint(BackupVaultName=<an existing vault name>, RecoveryPointArn=arn:aws:backup:<region>:<an account id that is NOT the caller's>:recovery-point:<uuid>).
2. Observe HTTP 403 (correct status and error class) with body $.Message = 'Insufficient privileges to perform this action.'
3. The recording answers HTTP 403 with $.Message = 'User: <the caller's principal ARN> is not authorized to perform: backup:DescribeRecoveryPoint on resource: <the RecoveryPointArn from the request path> because no resource-based policy allows the backup:DescribeRecoveryPoint action'.
4. The same divergence reproduces via fixture backup-recovery-point-error-paths-on-live-vault, step task#4 (cross-account ARN against a live vault).
5. Control: DescribeRecoveryPoint for an ARN in the caller's OWN account that does not exist must still answer 400 ResourceNotFoundException 'Cannot find recovery point', not this 403.
