Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds__s3.cli-rds-start-export-task-workflow
Issue-Key: rds__s3.cli-rds-start-export-task-workflow|_run_aws_with_retry#9|AssertionError|raise AssertionError(f"aws CLI failed (rc={proc.returncode}): {proc.stderr}")
Issue-Commit: 8fe1b88606c77dfa59d771b1a110885b6dfaed56

# rds__s3: StartExportTask is an unimplemented scaffold stub, so `aws rds start-export-task` is refused with NotImplemented (1 cause)

> _Note: this issue was identified against the code base at commit `8fe1b88606c77dfa59d771b1a110885b6dfaed56`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExportTask

## Description
ONE root cause. 1 CAUSE must be closed before this issue is submitted.

NOTE ON PRIOR ART: the open issue 20261024T030130Z-rds-s3-rds-snapshot-export-to-s3-and-restore-from.md names this check id in its Issue-Trace-Ids, but its stored replay key for it (`_wait_for#7|Timeout|time.sleep(interval_s)`) no longer matches — the check now fails EARLIER, on the very first `aws rds start-export-task` call, with an AssertionError carrying the CLI's NotImplemented error instead of timing out while waiting for a terminal task. The replay suite therefore did not mark this failure as OWNED, so the fix is currently un-gated for this check. This issue re-grounds the same cause on the current replay key.

Cause 1 of 1: StartExportTask answers HTTP 501 with error code NotImplemented, so snapshot/cluster export to S3 cannot be started at all. Failing check: rds__s3.cli-rds-start-export-task-workflow.

AWS: Smithy (services/service-com-amazonaws-rds-amazonrdsv19) models StartExportTask with required ExportTaskIdentifier, SourceArn, S3BucketName, IamRoleArn and KmsKeyId plus optional S3Prefix and ExportOnly, returning the ExportTask shape {ExportTaskIdentifier, SourceArn, ExportOnly, SnapshotTime, TaskStartTime, TaskEndTime, S3Bucket, S3Prefix, IamRoleArn, KmsKeyId, Status, PercentProgress, TotalExtractedDataInGB, FailureCause, WarningMessage, SourceType}. Status is documented as one of CANCELED | CANCELING | COMPLETE | FAILED | IN_PROGRESS | STARTING, so a well-formed request must come back STARTING/IN_PROGRESS and be observable through DescribeExportTasks (paginated on Marker, with ExportTaskNotFoundFault — client, 404, awsQueryError code ExportTaskNotFound — for an unknown identifier) until it reaches a terminal status. The modeled refusals are DBClusterNotFoundFault (404), DBClusterSnapshotNotFoundFault (404), DBSnapshotNotFoundFault (404, awsQueryError code DBSnapshotNotFound), ExportTaskAlreadyExistsFault (400, code ExportTaskAlreadyExists), IamRoleMissingPermissionsFault (400, code IamRoleMissingPermissions), IamRoleNotFoundFault (404, code IamRoleNotFound), InvalidExportOnlyFault (400, code InvalidExportOnly), InvalidExportSourceStateFault (400, code InvalidExportSourceState), InvalidS3BucketFault (400, code InvalidS3BucketFault) and KMSKeyNotAccessibleFault (400, code KMSKeyNotAccessibleFault). NotImplemented is not among them and is not a modeled outcome of this operation at all.

Emulator: every StartExportTask request is refused. The CLI reports `An error occurred (NotImplemented) when calling the StartExportTask operation: operation StartExportTask is scaffolded but not implemented in this emulator` (rc=254). The handler is still the generated scaffold — services/service-com-amazonaws-rds-amazonrdsv19/src/ops/start_export_task.rs carries the `//! SCAFFOLDED, NOT IMPLEMENTED` marker and its `handle` body is `Err(crate::scaffolded::Scaffolded::not_implemented("StartExportTask"))`.

Why: no client can export a DB snapshot or cluster to S3, and because the refusal is a 501 with an unmodeled code, SDK error handling cannot map it to anything: callers that branch on DBSnapshotNotFound, InvalidS3BucketFault, IamRoleNotFound/IamRoleMissingPermissions or KMSKeyNotAccessibleFault get an unrecognised Receiver fault instead, and retry logic treats it as a transient server error. The whole export surface — DescribeExportTasks progression, the .parquet objects under the bucket prefix, and every validation path an exporter depends on — is unreachable.

Fix: the StartExportTask handler (services/service-com-amazonaws-rds-amazonrdsv19/src/ops/start_export_task.rs — the only file to touch for this operation; delete the SCAFFOLDED marker when done), plus the export-task record in the RDS service state that DescribeExportTasks reads and the S3 objects the task must write under S3BucketName/S3Prefix, and the modeled refusals above for a missing snapshot, a missing/unauthorized IAM role, an inaccessible KMS key and a duplicate ExportTaskIdentifier.

Traces: rds__s3.cli-rds-start-export-task-workflow (frames run() line 35 -> `_run_aws_with_retry` line 91).

## Evidence
Expected (AWS): `aws rds start-export-task --export-task-identifier <id> --source-arn <snapshot arn> --s3-bucket-name <bucket> --iam-role-arn <role> --kms-key-id <cmk>` returns an ExportTask JSON document with Status "STARTING" (later IN_PROGRESS then COMPLETE) and the request echoed back in S3Bucket/S3Prefix/IamRoleArn/KmsKeyId/SourceArn/SourceType; DescribeExportTasks then reports the task until it is terminal.

Actual (emulator):
  [mismatch] rds__s3.cli-rds-start-export-task-workflow
  File "check_rds__s3_cli_rds_start_export_task_workflow.py", line 35, in run -> stdout = _run_aws_with_retry(...)
  File "...", line 91, in _run_aws_with_retry -> raise AssertionError(f"aws CLI failed (rc={proc.returncode}): {proc.stderr}")
  AssertionError: aws CLI failed (rc=254):
  aws: [ERROR]: An error occurred (NotImplemented) when calling the StartExportTask operation: operation StartExportTask is scaffolded but not implemented in this emulator

Source of the refusal: services/service-com-amazonaws-rds-amazonrdsv19/src/ops/start_export_task.rs line 4 (`//! SCAFFOLDED, NOT IMPLEMENTED`) and line 245 (`Err(crate::scaffolded::Scaffolded::not_implemented("StartExportTask"))`).

Smithy: StartExportTask -> ExportTask{..., Status in CANCELED|CANCELING|COMPLETE|FAILED|IN_PROGRESS|STARTING}; modeled errors DBClusterNotFoundFault 404, DBClusterSnapshotNotFoundFault 404, DBSnapshotNotFoundFault 404 (DBSnapshotNotFound), ExportTaskAlreadyExistsFault 400, IamRoleMissingPermissionsFault 400, IamRoleNotFoundFault 404, InvalidExportOnlyFault 400, InvalidExportSourceStateFault 400, InvalidS3BucketFault 400, KMSKeyNotAccessibleFault 400. NotImplemented is not modeled for this operation.

## Steps to Reproduce
1. Boot the emulator from mainline.
2. Create a DB instance and a DB snapshot, an S3 bucket, an IAM role for the export and a KMS CMK.
3. Run `aws rds start-export-task --export-task-identifier <id> --source-arn <snapshot arn> --s3-bucket-name <bucket> --iam-role-arn <role arn> --kms-key-id <cmk>` (or POST Action=StartExportTask): the response is HTTP 501 error code NotImplemented, "operation StartExportTask is scaffolded but not implemented in this emulator", instead of an ExportTask with Status STARTING.
4. Replay: rds__s3.cli-rds-start-export-task-workflow.
