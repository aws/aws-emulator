Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds__s3.rds-export-task-kms-key-missing-permission-negative rds__s3.rds-export-task-parquet-format-schema-preserved
Issue-Key: rds__s3.rds-export-task-kms-key-missing-permission-negative|_wait_for#18|Timeout|time.sleep(interval_s)
Issue-Key: rds__s3.rds-export-task-parquet-format-schema-preserved|_poll_until#26|Timeout|return last
Issue-Commit: 2352e453041dca9d0b463884f1fb4473cbd545be

# rds__s3: the snapshot-export lifecycle never settles — neither the KMS-denial refusal nor the Parquet-schema export reaches a verdict before the replay budget ends

> _Note: this issue was identified against the code base at commit `2352e453041dca9d0b463884f1fb4473cbd545be`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExportTask

## Description
Two causes, both in the RDS snapshot-export-to-S3 lifecycle. Each check builds its own VPC, DB instance and snapshot and then polls a resource for a terminal state with its own generous deadline (1200s for the instance, 900s for the snapshot, 1800-2400s for the export task); against this emulator neither check produces a single graded assertion before the suite's budget ends, so the emulator leaves one of those resources non-terminal for the whole run. Nothing in the replay output shows a refusal or a wrong field — the only observable is that the lifecycle never advances.

Cause 1 of 2: StartExportTask with a KMS key whose policy denies kms:CreateGrant and kms:DescribeKey is never refused, and the permitted-key control export never completes. Failing check id: rds__s3.rds-export-task-kms-key-missing-permission-negative.
AWS: Smithy (service-com-amazonaws-rds-amazonrdsv19) models `KMSKeyNotAccessibleFault` on StartExportTask, and `DescribeExportTasks` reports a `Status` that settles on COMPLETE / FAILED / CANCELED; `DescribeDBInstances` reports DBInstanceStatus and `DescribeDBSnapshots` a snapshot Status that reach "available". Recorded behavior: the denied-key StartExportTask is refused with Error.Code exactly `KMSKeyNotAccessibleFault` and writes nothing to the destination prefix, while the same snapshot exported under a permitted key reaches Status `COMPLETE` with objects present under the prefix.
Emulator: `Check timed out after 60.0s`. The check blocks in its lifecycle waits (`_wait_for(... DBInstanceStatus == "available", timeout_s=1200)`, then the snapshot wait, then `_wait_export_terminal`), so the KMSKeyNotAccessibleFault assertion at line `assert error["Code"] == "KMSKeyNotAccessibleFault"` and the COMPLETE assertion on the control task are both never reached.

Cause 2 of 2: the Parquet export of a table created over the native MySQL wire protocol never completes, so the exported schema is unobservable. Failing check id: rds__s3.rds-export-task-parquet-format-schema-preserved.
AWS: recorded behavior — after `_wait_instance_available` reports DBInstanceStatus "available" and the snapshot reaches "available", StartExportTask settles with DescribeExportTasks Status `COMPLETE` and writes Parquet objects under the requested S3 prefix whose column names and (loosely) types match the source table.
Emulator: `Check timed out after 60.0s`, blocked in the same chain — `_wait_instance_available(ctx, db_id)` (DBInstanceStatus == "available", timeout_s=1800, interval 20s) before the export is even started, then `_wait_export_task` (Status in {COMPLETE, FAILED, CANCELED}). `assert task["Status"] == "COMPLETE"` is never evaluated and no Parquet footer is ever read.

Why: a client that provisions a DB instance, snapshots it and exports the snapshot — the documented RDS-to-S3 analytics path — polls DescribeDBInstances / DescribeDBSnapshots / DescribeExportTasks until each is terminal. Against this emulator those polls do not terminate, so the caller hangs rather than getting an export (or a KMSKeyNotAccessibleFault it can handle), and no caller can test either the success or the refusal branch of its export code.

Fix: the RDS resource state machines these reads serve — the DBInstance status progression behind DescribeDBInstances, the snapshot status behind DescribeDBSnapshots, and the export-task progression behind StartExportTask/DescribeExportTasks — must advance to their terminal values ('available', and COMPLETE/FAILED/CANCELED) promptly, and StartExportTask's validation must refuse an inaccessible KMS key with the modelled KMSKeyNotAccessibleFault before any object is written.

Related open issues, which the report did NOT mark as owning these two checks: 20261112T074631Z-rds-s3-the-whole-snapshot-export-to-s3.md, 20261117T083457Z-rds-s3-2-net-new-causes-startexporttask-reaches.md and 20261113T183248Z-rds-s3-5-net-new-causes-in-startexporttask.md cover other StartExportTask divergences; a fixer should check whether this lifecycle stall is a distinct gap from those.

Traces: the two failing check ids listed above.

## Evidence
Expected (AWS, from the recording and the model):
- rds-export-task-kms-key-missing-permission-negative: StartExportTask with the deny-policy KMS key is refused with Error.Code exactly `KMSKeyNotAccessibleFault` and no object appears under the prefix; the same snapshot exported with a permitted key reaches DescribeExportTasks Status `COMPLETE` with objects present.
- rds-export-task-parquet-format-schema-preserved: DescribeDBInstances reports DBInstanceStatus "available" and an Endpoint, DescribeDBSnapshots reports Status "available", and DescribeExportTasks settles on Status `COMPLETE` with Parquet objects under the prefix whose columns match the source table.

Actual (emulator): both checks end with `Check timed out after 60.0s` and NO graded assertion. Neither reaches `assert error["Code"] == "KMSKeyNotAccessibleFault"` nor `assert task["Status"] == "COMPLETE"`; the run is spent inside the checks' own terminal-state polls (`_wait_for(..., DBInstanceStatus == "available", timeout_s=1200/1800, interval 20-30s)`, `_wait_for(..., snapshot Status == "available", timeout_s=900)`, `_wait_export_terminal/_wait_export_task(..., Status in {COMPLETE, FAILED, CANCELED})`). The instance/snapshot/task identifiers and bucket names are per-run values; the requirement is that each of those statuses reaches its terminal value and that the denied-key export is refused with the modelled fault code.

## Steps to Reproduce
1. Create a VPC, subnets and a DB subnet group, plus a security group allowing MySQL ingress.
2. CreateDBInstance (engine mysql) and poll DescribeDBInstances for DBInstanceStatus == "available" — AWS reaches it; the emulator does not within the budget, which already ends the Parquet check.
3. CreateDBSnapshot and poll DescribeDBSnapshots for Status == "available".
4. Create a KMS key whose policy DENIES kms:CreateGrant and kms:DescribeKey to the export role, and call StartExportTask with it — AWS refuses with Error.Code `KMSKeyNotAccessibleFault` and writes nothing.
5. Call StartExportTask again with a permitted key and poll DescribeExportTasks for Status in {COMPLETE, FAILED, CANCELED} — AWS settles on COMPLETE with objects under the prefix; the emulator's poll never terminates.
6. For the Parquet check, create the table over the native MySQL wire protocol before snapshotting, then read the exported Parquet footer and compare column names and types with the source table.
