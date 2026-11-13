Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds__s3.cfn-deploy-rds-missing-s3-export-role-negative rds__s3.cli-rds-export-task-missing-s3-bucket-policy-negative rds__s3.cli-rds-export-task-missing-role-arn-negative rds__s3.cli-rds-export-task-missing-role-negative rds__s3.rds-export-task-kms-key-missing-permission-negative rds__s3.rds-s3-export-kms-key-mismatch-negative rds__s3.rds-export-task-s3-bucket-policy-denies-negative rds__s3.cli-rds-export-invalid-bucket-name-negative rds__s3.deploy-cfn-rds-s3-export-role-missing-trust-negative rds__s3.deploy-cfn-rds-s3-role-wrong-service-principal-negative rds__s3.iam-role-trust-policy-rds-s3-positive rds__s3.rds-export-task-cross-region-bucket-rejected rds__s3.rds-export-task-nonexistent-bucket-negative rds__s3.rds-export-task-nonexistent-bucket-rejected rds__s3.rds-export-task-role-trust-wrong-service-principal-negative rds__s3.rds-export-task-role-wrong-trust-principal-rejected rds__s3.rds-export-task-wrong-bucket-name-negative rds__s3.rds-iam-role-trust-wrong-principal-negative
Issue-Key: rds__s3.cfn-deploy-rds-missing-s3-export-role-negative|run#20|AssertionError|assert message == (
Issue-Key: rds__s3.cli-rds-export-task-missing-s3-bucket-policy-negative|run#21|AssertionError|assert message == (
Issue-Key: rds__s3.cli-rds-export-task-missing-role-arn-negative|run#32|AssertionError|assert code == "KMSKeyNotAccessibleFault", f"expected KMSKeyNotAccessibleFault, got {code}"
Issue-Key: rds__s3.cli-rds-export-task-missing-role-negative|run#18|AssertionError|assert (code, message) == ("KMSKeyNotAccessibleFault", expected), (
Issue-Key: rds__s3.rds-export-task-kms-key-missing-permission-negative|run#13|AssertionError|assert error["Message"] == expected_message, (
Issue-Key: rds__s3.rds-s3-export-kms-key-mismatch-negative|run#15|AssertionError|assert error["Message"] == expected_message, (
Issue-Key: rds__s3.rds-export-task-s3-bucket-policy-denies-negative|_client_error#14|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: rds__s3.cli-rds-export-invalid-bucket-name-negative|_poll_until#21|Timeout|time.sleep(interval_s)
Issue-Key: rds__s3.deploy-cfn-rds-s3-export-role-missing-trust-negative|_poll_until#28|Timeout|return last
Issue-Key: rds__s3.deploy-cfn-rds-s3-role-wrong-service-principal-negative|_poll_until#28|Timeout|return last
Issue-Key: rds__s3.iam-role-trust-policy-rds-s3-positive|_wait_for#20|Timeout|return last
Issue-Key: rds__s3.rds-export-task-cross-region-bucket-rejected|_poll_until#26|Timeout|return last
Issue-Key: rds__s3.rds-export-task-nonexistent-bucket-negative|_poll_until#19|Timeout|time.sleep(interval_s)
Issue-Key: rds__s3.rds-export-task-nonexistent-bucket-rejected|_poll_until#21|Timeout|return last
Issue-Key: rds__s3.rds-export-task-role-trust-wrong-service-principal-negative|_poll_until#20|Timeout|time.sleep(interval_s)
Issue-Key: rds__s3.rds-export-task-role-wrong-trust-principal-rejected|_poll_until#23|Timeout|return last
Issue-Key: rds__s3.rds-export-task-wrong-bucket-name-negative|_poll_until#22|Timeout|return last
Issue-Key: rds__s3.rds-iam-role-trust-wrong-principal-negative|_poll_until#18|Timeout|time.sleep(interval_s)
Issue-Commit: f2f8f36b8a9d5e45c8e23580863863f823670049

# rds__s3: 5 net-new causes in StartExportTask authorization — the S3 and KMS refusal messages name the wrong action, a nonexistent role answers the wrong error class, a bucket-policy deny is not evaluated at all, and eleven export checks never settle

> _Note: this issue was identified against the code base at commit `f2f8f36b8a9d5e45c8e23580863863f823670049`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExportTask

## Description
5 net-new root causes in the RDS snapshot-export-to-S3 authorization surface. Related open issues cover neighbouring causes but carry NONE of these replay keys: 20261109T135215Z-rds-s3-9-net-new-causes-export-task.md (its cause 3 is the same wrong-S3-action wording, under different checks; its cause 2 covered the bogus-role case when the call still SUCCEEDED, which it no longer does), 20261110T153718Z-rds-s3-the-snapshot-export-to-s3-path.md, 20261112T072527Z-rds-s3-the-rds-snapshot-export-to-s3.md, 20261112T074631Z-rds-s3-the-whole-snapshot-export-to-s3.md.

Cause 1 of 5: the IamRoleMissingPermissions message names s3:PutObject where AWS names the action that actually failed, s3:GetBucketLocation.
AWS: when the export role cannot reach the destination bucket at all (no S3 grant, or a bucket policy that denies it outright), StartExportTask is refused 400 IamRoleMissingPermissions with "The IAM role <role-arn> isn't authorized to call s3:GetBucketLocation on the S3 bucket <bucket>." — fixed text, with the role ARN and bucket name taken from the request; GetBucketLocation is the first call RDS makes, so that is the action named. (When only the write is denied, the same template names s3:PutObject — the action named is the probe that failed, not a constant.) Smithy: StartExportTask error IamRoleMissingPermissionsFault, awsQueryError IamRoleMissingPermissions, HTTP 400; the message text is recorded AWS behavior.
Emulator: the same sentence but always with s3:PutObject, even where the role has no S3 permissions whatsoever and GetBucketLocation is what AWS reports.
Why: an operator fixes the wrong IAM/bucket-policy statement — granting s3:PutObject does not make the export work, because the call that failed was GetBucketLocation — and a client matching AWS's sentence matches nothing.
Fix: the authorization-probe order and the message template in the StartExportTask handler of the rds service.
Traces: rds__s3.cfn-deploy-rds-missing-s3-export-role-negative, rds__s3.cli-rds-export-task-missing-s3-bucket-policy-negative.

Cause 2 of 5: a syntactically valid IamRoleArn naming no existing role is refused IamRoleNotFound where AWS answers KMSKeyNotAccessibleFault.
AWS: StartExportTask resolves the KMS grant THROUGH the named role first, so a request whose IamRoleArn does not exist is refused 400 KMSKeyNotAccessibleFault with "The specified KMS key [<key-ref-as-passed>] does not exist, is not enabled or you do not have permissions to access it." — fixed text with the KmsKeyId exactly as the request spelled it (ARN or key id) in the brackets. Smithy: StartExportTask errors KMSKeyNotAccessibleFault (400) and IamRoleNotFoundFault (404) are both modeled; which one AWS answers for this input is recorded behavior.
Emulator: 404 IamRoleNotFound with 'The principal export.rds.amazonaws.com isn't allowed to assume the IAM role <arn> or the IAM role <arn> doesn't exist.'
Why: a client's error handling branches on the code — AWS's answer sends it to the KMS grant, the emulator's to IAM — and a 404 where AWS returns 400 changes retry classification.
Fix: the validation order in the StartExportTask handler (resolve the KMS key through the role before reporting a role-resolution failure).
Traces: rds__s3.cli-rds-export-task-missing-role-arn-negative, rds__s3.cli-rds-export-task-missing-role-negative.

Cause 3 of 5: a KMS key the export role cannot use is reported as an IAM authorization failure instead of AWS's KMS wording.
AWS: when the KMS key's policy denies the export role kms:CreateGrant/kms:DescribeKey (or the key is otherwise unusable), StartExportTask is refused 400 KMSKeyNotAccessibleFault worded as absence: "The KMS key <key-ref-as-passed> doesn't exist or is disabled." — fixed text, echoing the KmsKeyId exactly as the request passed it (key id or ARN), and naming neither the role nor the KMS action. Smithy: error KMSKeyNotAccessibleFault, awsQueryError KMSKeyNotAccessibleFault, HTTP 400; the message is recorded AWS behavior.
Emulator: the code is right but the message is 'The IAM role <role-arn> isn't authorized to call kms:CreateGrant on the KMS key <key>.' — an emulator-authored IAM sentence.
Why: a client matching or parsing AWS's sentence matches nothing, and the message points the operator at the role's identity policy when AWS deliberately words this as a key-state problem.
Fix: the KMS-grant check's message template in the StartExportTask handler.
Traces: rds__s3.rds-export-task-kms-key-missing-permission-negative, rds__s3.rds-s3-export-kms-key-mismatch-negative.

Cause 4 of 5: a destination bucket policy that denies the export role is not evaluated, so the export is accepted.
AWS: RDS probes the role's EFFECTIVE S3 access (identity policy AND bucket policy) at StartExportTask time, before any task exists, so a role denied by the bucket policy is refused synchronously 400 IamRoleMissingPermissions with "The IAM role <role-arn> isn't authorized to call s3:PutObject on the S3 bucket <bucket>." and no export task is created and no object is written. Smithy: IamRoleMissingPermissionsFault, HTTP 400.
Emulator: the call answers HTTP 200 and returns an ExportTask with Status 'STARTING' for the denied role, so a pollable task exists where AWS created none.
Why: a negative test cannot pass, and worse, a misconfigured export appears to start: the caller waits on a task that can never write the data and gets no error explaining why.
Fix: the effective-access probe in the StartExportTask handler (consult the destination bucket's policy, not only the role's identity policy).
Traces: rds__s3.rds-export-task-s3-bucket-policy-denies-negative.

Cause 5 of 5: eleven export/role checks never settle inside the replay budget.
AWS: StartExportTask's refusals are synchronous and a permitted export reaches a terminal Status (COMPLETE) with its objects in S3, all well inside a minute against a local service; every positive control in these checks waits on DescribeExportTasks reaching COMPLETE/FAILED/CANCELED.
Emulator: all eleven checks are killed by the 60 s replay budget ('Check timed out after 60.0s') with no refusal and no terminal export Status recorded (recorded behavior only) — the export-task state machine does not advance to a terminal Status, so each check's terminal-status poll (allowed up to 1800 s) consumes the budget. Two of these ids (rds__s3.rds-export-task-wrong-bucket-name-negative, rds__s3.rds-iam-role-trust-wrong-principal-negative) are named in open issue 20261109T135215Z but now fail earlier, with this timeout instead of the message assertion recorded there.
Why: a client polling DescribeExportTasks waits forever and gets no error to act on, and no refusal path in this family can be observed at all.
Fix: the export-task state machine in the rds service — progress a started task to a terminal Status (and write its objects) so a poll terminates.
Traces: rds__s3.cli-rds-export-invalid-bucket-name-negative, rds__s3.deploy-cfn-rds-s3-export-role-missing-trust-negative, rds__s3.deploy-cfn-rds-s3-role-wrong-service-principal-negative, rds__s3.iam-role-trust-policy-rds-s3-positive, rds__s3.rds-export-task-cross-region-bucket-rejected, rds__s3.rds-export-task-nonexistent-bucket-negative, rds__s3.rds-export-task-nonexistent-bucket-rejected, rds__s3.rds-export-task-role-trust-wrong-service-principal-negative, rds__s3.rds-export-task-role-wrong-trust-principal-rejected, rds__s3.rds-export-task-wrong-bucket-name-negative, rds__s3.rds-iam-role-trust-wrong-principal-negative.

## Evidence
Cause 1: expected "The IAM role <role-arn> isn't authorized to call s3:GetBucketLocation on the S3 bucket <bucket>."; got "The IAM role arn:aws:iam::000000000000:role/sim2real-xsvc-stk-...-BadRole-... isn't authorized to call s3:PutObject on the S3 bucket sim2real-xsvc-noperm-bkt-..." (and the same substitution in the CLI bucket-policy check).
Cause 2: expected KMSKeyNotAccessibleFault with 'The specified KMS key [<kms-arn>] does not exist, is not enabled or you do not have permissions to access it.'; got IamRoleNotFound with 'The principal export.rds.amazonaws.com isn't allowed to assume the IAM role <arn> or the IAM role <arn> doesn't exist.'
Cause 3: expected "The KMS key <key-id> doesn't exist or is disabled."; got "The IAM role <role-arn> isn't authorized to call kms:CreateGrant on the KMS key <key-id>."
Cause 4: expected the call to be rejected, but it succeeded: {'ExportTaskIdentifier': 'sim2real-rds-bucketdeny-bad-...', 'Status': 'STARTING', 'PercentProgress': 0, ... 'HTTPStatusCode': 200}.
Cause 5: Error: Check timed out after 60.0s (eleven checks).

## Steps to Reproduce
Cause 1: StartExportTask with a role that has the right trust but no S3/KMS policy (or whose destination bucket policy denies it entirely) and read the Message.
Cause 2: StartExportTask with a syntactically valid IamRoleArn that names no existing role, plus a usable KmsKeyId, and read the Code and Message.
Cause 3: StartExportTask with a KMS key whose policy denies the export role kms:CreateGrant/kms:DescribeKey and read the Message.
Cause 4: StartExportTask with a role whose identity policy allows the write but whose destination bucket policy denies it; observe the 200/STARTING response and that DescribeExportTasks lists the task.
Cause 5: run any of the eleven listed export/role checks and observe that no refusal and no terminal export Status is reached inside the budget.
