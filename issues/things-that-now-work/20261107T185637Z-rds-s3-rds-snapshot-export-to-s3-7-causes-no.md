Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds__s3.rds-export-manifest-json-format-preserved rds__s3.rds-export-manifest-format-preserved rds__s3.rds-export-task-creates-s3-objects-positive rds__s3.cli-rds-export-task-s3-object-listing-verify rds__s3.cli-rds-start-export-task-workflow rds__s3.rds-snapshot-export-to-s3-positive rds__s3.rds-export-manifest-file-format-positive rds__s3.cli-rds-create-export-task-verify-s3-sdk rds__s3.rds-snapshot-export-task-manifest-file-in-s3 rds__s3.rds-snapshot-export-task-writes-to-s3-positive rds__s3.rds-export-task-parquet-format-schema-preserved rds__s3.rds-export-task-nonexistent-bucket-negative rds__s3.rds-export-task-nonexistent-bucket-rejected rds__s3.cli-rds-export-invalid-bucket-name-negative rds__s3.cfn-deploy-rds-s3-export-bucket-missing-negative rds__s3.rds-snapshot-export-nonexistent-bucket-negative rds__s3.rds-snapshot-export-task-wrong-bucket-arn-negative rds__s3.rds-export-task-cross-region-bucket-rejected rds__s3.rds-export-task-role-trust-missing-negative rds__s3.rds-export-task-role-trust-wrong-service-principal-negative rds__s3.rds-export-task-role-wrong-trust-principal-rejected rds__s3.deploy-cfn-rds-s3-export-role-missing-trust-negative rds__s3.deploy-cfn-rds-s3-role-wrong-service-principal-negative rds__s3.deploy-rds-s3-export-wrong-trust-policy-negative rds__s3.iam-role-trust-policy-rds-s3-positive rds__s3.rds-snapshot-export-missing-iam-role-negative rds__s3.cfn-deploy-rds-missing-s3-export-role-negative rds__s3.cli-rds-export-task-missing-s3-bucket-policy-negative rds__s3.deploy-rds-s3-export-stack-missing-role-negative rds__s3.rds-snapshot-export-task-missing-s3-put-permission-negative rds__s3.rds-export-task-kms-key-missing-permission-negative rds__s3.rds-export-task-s3-bucket-policy-denies-negative rds__s3.rds-s3-export-kms-key-mismatch-negative rds__s3.deploy-cfn-rds-s3-bucket-policy-restricts-export-negative rds__s3.deploy-s3-bucket-policy-grants-rds-export-negative rds__s3.cli-rds-export-task-missing-role-negative
Issue-Key: rds__s3.rds-export-manifest-json-format-preserved|run#13|AssertionError|assert parquet_keys, f"expected parquet objects under base prefix, got: {[o['Key'] for o in objects]}"
Issue-Key: rds__s3.rds-export-manifest-format-preserved|run#15|AssertionError|assert matching_base, (
Issue-Key: rds__s3.rds-export-task-creates-s3-objects-positive|run#15|AssertionError|assert parquet_objects, f"no .parquet objects among exported keys: {[o['Key'] for o in objects]}"
Issue-Key: rds__s3.cli-rds-export-task-s3-object-listing-verify|run#16|AssertionError|assert any(ln.lower().endswith(".parquet") for ln in lines), (
Issue-Key: rds__s3.cli-rds-start-export-task-workflow|run#18|AssertionError|assert any(k.endswith(".parquet") for k in keys), f"no parquet objects found: {keys[:10]}"
Issue-Key: rds__s3.rds-snapshot-export-to-s3-positive|run#14|AssertionError|assert parquet_keys, f"no .parquet objects found among exported keys: {keys[:10]}"
Issue-Key: rds__s3.rds-export-manifest-file-format-positive|run#15|AssertionError|assert info_key in all_keys, (
Issue-Key: rds__s3.cli-rds-create-export-task-verify-s3-sdk|run#23|AssertionError|assert manifest_key in keys, f"expected RDS export manifest {manifest_key!r} not found among: {keys!r}"
Issue-Key: rds__s3.rds-snapshot-export-task-manifest-file-in-s3|run#16|AssertionError|assert manifest_key in keys, (
Issue-Key: rds__s3.rds-snapshot-export-task-writes-to-s3-positive|run#17|AssertionError|assert any(k.endswith(f"export_info_{export_id}.json") for k in matching), (
Issue-Key: rds__s3.rds-export-task-parquet-format-schema-preserved|run#35|AssertionError|assert "decimal" in score_type, f"score column exported as {score_type!r}, lost its decimal precision"
Issue-Key: rds__s3.rds-export-task-nonexistent-bucket-negative|run#11|AssertionError|assert error.get("Message") == expected_message, (
Issue-Key: rds__s3.rds-export-task-nonexistent-bucket-rejected|run#14|AssertionError|assert error.get("Message") == expected_message, (
Issue-Key: rds__s3.cli-rds-export-invalid-bucket-name-negative|run#15|AssertionError|assert bad_proc.stderr.strip() == expected_stderr, (
Issue-Key: rds__s3.cfn-deploy-rds-s3-export-bucket-missing-negative|run#22|AssertionError|assert message == (
Issue-Key: rds__s3.rds-snapshot-export-nonexistent-bucket-negative|run#11|AssertionError|assert message == (
Issue-Key: rds__s3.rds-snapshot-export-task-wrong-bucket-arn-negative|run#12|AssertionError|assert message == (
Issue-Key: rds__s3.rds-export-task-cross-region-bucket-rejected|run#18|AssertionError|assert error.get("Message") == expected_message, (
Issue-Key: rds__s3.rds-export-task-role-trust-missing-negative|run#11|AssertionError|assert error.get("Message") == expected_message, (
Issue-Key: rds__s3.rds-export-task-role-trust-wrong-service-principal-negative|run#11|AssertionError|assert error.get("Message") == expected_message, (
Issue-Key: rds__s3.rds-export-task-role-wrong-trust-principal-rejected|run#14|AssertionError|assert error.get("Message") == expected_message, (
Issue-Key: rds__s3|StartExportTask|IamRoleNotFound
Issue-Key: rds__s3.iam-role-trust-policy-rds-s3-positive|run#12|AssertionError|assert message == expected, message
Issue-Key: rds__s3.rds-snapshot-export-missing-iam-role-negative|run#11|AssertionError|assert message == (
Issue-Key: rds__s3.cfn-deploy-rds-missing-s3-export-role-negative|run#20|AssertionError|assert message == (
Issue-Key: rds__s3.cli-rds-export-task-missing-s3-bucket-policy-negative|run#21|AssertionError|assert message == (
Issue-Key: rds__s3.deploy-rds-s3-export-stack-missing-role-negative|run#21|AssertionError|assert message in {
Issue-Key: rds__s3.rds-snapshot-export-task-missing-s3-put-permission-negative|run#10|AssertionError|assert message == (
Issue-Key: rds__s3.rds-export-task-kms-key-missing-permission-negative|_client_error#10|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: rds__s3.rds-export-task-s3-bucket-policy-denies-negative|_client_error#14|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: rds__s3.rds-s3-export-kms-key-mismatch-negative|_start_export_refusal#12|AssertionError|raise AssertionError(f"expected StartExportTask to be refused, but it was accepted: {result!r}")
Issue-Key: rds__s3.deploy-cfn-rds-s3-bucket-policy-restricts-export-negative|run#20|AssertionError|raise AssertionError("start_export_task to a bucket that denies the role principal unexpectedly succeeded")
Issue-Key: rds__s3.deploy-s3-bucket-policy-grants-rds-export-negative|run#26|AssertionError|raise AssertionError(
Issue-Key: rds__s3.cli-rds-export-task-missing-role-negative|run#14|AssertionError|assert proc.returncode != 0, (
Issue-Commit: d6cb6c274eaec5e2dce099ad9d2ddf00c4faf8ea

# RDS snapshot export to S3: 7 causes — no Parquet data, no export_info manifest, lost decimal precision, and four divergent refusal classes

> _Note: this issue was identified against the code base at commit `d6cb6c274eaec5e2dce099ad9d2ddf00c4faf8ea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExportTask

## Description
Seven root causes in the RDS snapshot-export-to-S3 path, all grounded in failing checks this report does NOT mark ALREADY FILED or OWNED. Open issues 20261029T195009Z-rds-s3-5-net-new-root-causes-startexporttask.md and 20261107T061500Z-rds-s3-6-remaining-causes-in-engine-s3-statements-and-cloudformation.md own OTHER checks of this service and do not carry the replay keys below. Smithy: StartExportTask errors InvalidS3BucketFault (400), IamRoleNotFoundFault (404), IamRoleMissingPermissionsFault (400), KMSKeyNotAccessibleFault (400).

Cause 1 of 7: the export writes no Parquet data files.
AWS: a COMPLETE export lays objects out as <export_identifier>/<database_name>/<schema_or_db>.<table>/<part>.parquet (gzip-compressed Parquet with the PAR1 magic at both ends); the data files are the point of the export. recorded behavior only for the file count.
Emulator: the only object ever written is <export_identifier>/export_tables_info_<export_identifier>_from_1_to_1.json — the checks report "expected parquet objects under base prefix, got: ['<task>/export_tables_info_<task>_from_1_to_1.json']", "no .parquet table file found among listed keys", "expected object keys under documented base prefix '<task>/mysql/mysql.help_topic/'". (The task ids are this run's.)
Why: an export produces no data; a client that exports a snapshot to query in Athena/Spark finds an empty dataset while the task says COMPLETE.
Fix: the RDS export-task executor's data writer (per-table Parquet output under the documented prefix layout).
Traces: rds__s3.rds-export-manifest-json-format-preserved, .rds-export-manifest-format-preserved, .rds-export-task-creates-s3-objects-positive, .cli-rds-export-task-s3-object-listing-verify, .cli-rds-start-export-task-workflow, .rds-snapshot-export-to-s3-positive.

Cause 2 of 7: the export_info_<export_identifier>.json summary manifest is never written.
AWS: a COMPLETE export writes <export_identifier>/export_info_<export_identifier>.json describing the export (tables, row/byte counts, status) alongside the per-table export_tables_info files.
Emulator: only export_tables_info_<task>_from_1_to_1.json exists — "no export_info summary object found among: [...]", "expected RDS export manifest '<task>/export_info_<task>.json' not found among: [...]".
Why: tooling that reads the export manifest to discover what was exported finds no manifest at all.
Fix: the RDS export-task executor's manifest writer.
Traces: rds__s3.rds-export-manifest-file-format-positive, .cli-rds-create-export-task-verify-s3-sdk, .rds-snapshot-export-task-manifest-file-in-s3, .rds-snapshot-export-task-writes-to-s3-positive.

Cause 3 of 7: exported Parquet loses decimal precision.
AWS: a DECIMAL/NUMERIC column is exported as a Parquet DECIMAL with the column's precision and scale, not as a floating-point type.
Emulator: the column is written as 'double' — "score column exported as 'double', lost its decimal precision".
Why: exported monetary/exact-numeric data silently loses exactness, so query results differ from the database.
Fix: the export writer's type mapping from the engine's column types to Parquet logical types.
Traces: rds__s3.rds-export-task-parquet-format-schema-preserved.

Cause 4 of 7: the InvalidS3BucketFault message text diverges.
AWS (400 InvalidS3BucketFault): "The S3 bucket <bucket-name> isn't valid. Either it isn't located in the current AWS Region or it doesn't exist." for a missing/invalid bucket, and "The S3 bucket <bucket-name> isn't located in the current AWS Region." for a bucket in another Region. Fixed text as quoted; the only variable part is the bucket name from the request.
Emulator: returns the same code with its own wording — "The specified Amazon S3 bucket name <bucket-name> can't be found or Amazon RDS isn't authorized to access the specified Amazon S3 bucket." and, cross-Region, "The S3 bucket <name> is in region us-east-1, which is not the region the export runs in (us-west-2). The S3 bucket must be in the same region as the snapshot or cluster being exported." (the region names are this run's values).
Why: callers and CLI users that match on the message — and the CLI's own stderr text — diverge from AWS, and the emulator's message conflates 'does not exist' with 'not authorized'.
Fix: the StartExportTask validator's InvalidS3BucketFault message construction.
Traces: rds__s3.rds-export-task-nonexistent-bucket-negative, .rds-export-task-nonexistent-bucket-rejected, .cli-rds-export-invalid-bucket-name-negative, .cfn-deploy-rds-s3-export-bucket-missing-negative, .rds-snapshot-export-nonexistent-bucket-negative, .rds-snapshot-export-task-wrong-bucket-arn-negative, .rds-export-task-cross-region-bucket-rejected.

Cause 5 of 7: the IamRoleNotFoundFault message text diverges.
AWS (404 IamRoleNotFound): "The principal export.rds.amazonaws.com isn't allowed to assume the IAM role <role-arn> or the IAM role <role-arn> doesn't exist." — fixed text with the service principal and the role ARN from the request.
Emulator: "The IAM role <role-arn> is missing for exporting to an Amazon S3 bucket. The role must exist and its trust relationship must allow export.rds.amazonaws.com to assume it."
Why: a caller cannot match AWS's documented message, and the emulator's wording claims the role is 'missing' when the real condition is a trust-policy mismatch.
Fix: the StartExportTask role-resolution validator's IamRoleNotFoundFault message construction.
Traces: rds__s3.rds-export-task-role-trust-missing-negative, .rds-export-task-role-trust-wrong-service-principal-negative, .rds-export-task-role-wrong-trust-principal-rejected, .deploy-cfn-rds-s3-export-role-missing-trust-negative, .deploy-cfn-rds-s3-role-wrong-service-principal-negative, .deploy-rds-s3-export-wrong-trust-policy-negative, .iam-role-trust-policy-rds-s3-positive, .rds-snapshot-export-missing-iam-role-negative.

Cause 6 of 7: the IamRoleMissingPermissionsFault message text diverges.
AWS (400 IamRoleMissingPermissions): the message names the role ARN and the missing S3 permission in AWS's own wording for that error class. recorded behavior only for the exact string; the recordings show AWS's text, which the emulator does not reproduce.
Emulator: "The IAM role <role-arn> requires additional permissions to export to the Amazon S3 bucket <bucket>: the role's policy must allow s3:PutObject on the objects of that bucket, and the bucket policy must not deny it." — an emulator-authored sentence rejected by every check that reads it.
Why: callers matching on AWS's message see a different one; the text also prescribes a remedy AWS does not state, hiding which of the two conditions actually failed.
Fix: the StartExportTask role-permission validator's IamRoleMissingPermissionsFault message construction.
Traces: rds__s3.cfn-deploy-rds-missing-s3-export-role-negative, .cli-rds-export-task-missing-s3-bucket-policy-negative, .deploy-rds-s3-export-stack-missing-role-negative, .rds-snapshot-export-task-missing-s3-put-permission-negative.

Cause 7 of 7: StartExportTask is accepted when the KMS key, the bucket policy or the role make the export impossible.
AWS: StartExportTask validates synchronously and refuses — 400 KMSKeyNotAccessibleFault when the caller/role cannot use the named CMK, IamRoleMissingPermissions/InvalidS3BucketFault when the destination bucket policy denies the role's PutObject, and IamRoleNotFound when the named role does not exist.
Emulator: the call returns 200 with Status 'STARTING' in every one of these cases — "expected the call to be refused, but it succeeded: {... 'Status': 'STARTING' ...}", "start_export_task to a bucket that denies the role principal unexpectedly succeeded", "start-export-task with a nonexistent IAM role must be rejected".
Why: a misconfigured export looks healthy at submit time and then produces nothing, so the caller gets no actionable error at the only point it could fix the configuration.
Fix: the StartExportTask handler's synchronous validation of the KMS key's usability, the destination bucket policy and the existence of the named role.
Traces: rds__s3.rds-export-task-kms-key-missing-permission-negative, .rds-export-task-s3-bucket-policy-denies-negative, .rds-s3-export-kms-key-mismatch-negative, .deploy-cfn-rds-s3-bucket-policy-restricts-export-negative, .deploy-s3-bucket-policy-grants-rds-export-negative, .cli-rds-export-task-missing-role-negative.

## Evidence
C1: expected '<task>/<db>/<db>.<table>/*.parquet'; got only ['<task>/export_tables_info_<task>_from_1_to_1.json'].
C2: expected '<task>/export_info_<task>.json'; not present.
C3: expected a Parquet DECIMAL with the column's precision/scale; got 'double'.
C4: expected "The S3 bucket <name> isn't valid. Either it isn't located in the current AWS Region or it doesn't exist."; got "The specified Amazon S3 bucket name <name> can't be found or Amazon RDS isn't authorized to access the specified Amazon S3 bucket."
C5: expected "The principal export.rds.amazonaws.com isn't allowed to assume the IAM role <arn> or the IAM role <arn> doesn't exist."; got "The IAM role <arn> is missing for exporting to an Amazon S3 bucket. ..."
C6: expected AWS's IamRoleMissingPermissions message; got "The IAM role <arn> requires additional permissions to export to the Amazon S3 bucket <bucket>: ..."
C7: expected 400 KMSKeyNotAccessibleFault / IamRoleMissingPermissions / IamRoleNotFound; got 200 with Status 'STARTING'.

## Steps to Reproduce
1) Seed a DB snapshot, a bucket and a CMK; create a role trusted by export.rds.amazonaws.com with s3:PutObject.
2) StartExportTask and wait for COMPLETE, then ListObjectsV2 under the task prefix (C1, C2) and inspect a Parquet file's schema (C3).
3) Repeat with: a nonexistent bucket and a bucket in another Region (C4); a role with no trust / the wrong service principal (C5); a role without s3:PutObject or a bucket policy that denies it (C6); a CMK the role cannot use, a denying bucket policy, and a nonexistent role (C7).
