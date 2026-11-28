Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena__emr-serverless__glue__s3.spark-saveastable-without-glue-createtable-permission-is-denied
Issue-Key: athena__emr-serverless__glue__s3.spark-saveastable-without-glue-createtable-permission-is-denied|run#31|AssertionError|assert (int(row[0]), int(row[1])) == (total_rows, total_amount), (
Issue-Commit: 929983b8776c4202c2a162ee50b02314bb9b729f

# Spark write with mode("overwrite") to an S3 warehouse location adds to the output already there instead of replacing it, so the Glue table read back by Athena double-counts every row

> _Note: this issue was identified against the code base at commit `929983b8776c4202c2a162ee50b02314bb9b729f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartJobRun

## Description
Cause 1 of 1: a second EMR Serverless Spark run that writes the SAME warehouse location with SaveMode.Overwrite does not replace what that location already holds, so the Glue table over it serves each row twice. Failing check: athena__emr-serverless__glue__s3.spark-saveastable-without-glue-createtable-permission-is-denied.

AWS: recorded behavior only — the Smithy model describes only the control plane (emr-serverless StartJobRun, POST /applications/{applicationId}/jobruns, and GetJobRun, GET /applications/{applicationId}/jobruns/{jobRunId}); it says nothing about Spark write semantics, so the recording is the authority. In the recording the submitted PySpark program writes with mode("overwrite").format("parquet").partitionBy("dt").option("path", <s3 warehouse uri>).saveAsTable("<db>.<tbl>"). Spark's overwrite save mode REPLACES the output already present at that destination path (the write deletes the existing files of the target path/partitions before committing the new ones), so after an earlier run of the same program left partial Parquet output under that prefix, a later overwrite run leaves exactly ONE copy of the input rows there. A following Athena query SELECT count(*), sum(<numeric column>) over the Glue table that points at that location therefore returns the seeded row count and the seeded sum exactly once — that equality (count == rows seeded, sum == sum of the seeded amounts) is the requirement, not the particular numbers of one run.

Emulator: the first job run (execution role deliberately missing glue:CreateTable) correctly ends FAILED and correctly registers no catalog table, but the Parquet output it had already written stays under the warehouse prefix. The second run, which differs ONLY by an execution role that also grants glue:CreateTable and which writes with the same mode("overwrite") to the same path, ends SUCCESS and registers the table with PartitionKeys [("dt","string")] — but its output is ADDED beside the first run's instead of replacing it. Athena then scans both copies and reports exactly double: in this replay it read ['180', '8814322'] where the seeded input was (90, 4407161), i.e. 2x rows and 2x sum (the per-run values are random; the doubling is the invariant). No error or warning accompanies it. Hypothesis: either the emulated Spark write path treats every write as an append (object keys are added, never pruned) rather than honouring SaveMode.Overwrite, or the Athena/Glue read over the table's location scans leftover objects from the earlier, failed run that the overwrite should have removed.

Why: a client that relies on an overwrite job being idempotent — a retry after a failed run, a scheduled full refresh of a partitioned table, any backfill — gets a silently duplicated dataset. Every aggregate computed over that table (counts, sums, averages, dedupe/uniqueness assumptions, downstream joins) is wrong, and because the run reports SUCCESS and the catalog entry looks correct there is nothing for the client to detect the corruption by.

Fix: the emulated Spark write in the emr-serverless job-execution path — the component that performs the submitted program's DataFrameWriter against the S3 location. It must honour the write's save mode: an overwrite write deletes the objects the destination path (and, for a partitioned write, the partitions it writes) already holds before committing its own output, instead of adding new object keys alongside them. The table-read side must likewise expose only the current snapshot of the location, not files a previous, failed run left behind.

Traces: athena__emr-serverless__glue__s3.spark-saveastable-without-glue-createtable-permission-is-denied (the first, denied run is graded correctly; the failure is on the positive control's Athena read).

## Evidence
Failing check athena__emr-serverless__glue__s3.spark-saveastable-without-glue-createtable-permission-is-denied, replay verdict [mismatch]:

  File "check_athena__emr_serverless__glue__s3_spark_saveastable_without_glue_createtable_permission_is_denied.py", line 465, in run
    assert (int(row[0]), int(row[1])) == (total_rows, total_amount), (
  AssertionError: Athena reads ['180', '8814322'] from the control table; the seeded input has (90, 4407161)

Expected (real AWS): after the control run (the same job, with glue:CreateTable granted) writes the partitioned table with mode("overwrite") to the warehouse prefix, Athena's SELECT count(*) AS c, sum(amount) AS s over "<db>"."<tbl>" returns the seeded row count and the seeded sum exactly once — here (90, 4407161).

Actual (emulator): Athena returns exactly twice the seeded values — (180, 8814322) — i.e. the output of the earlier FAILED run is still at the warehouse location and the overwrite write added a second copy beside it instead of replacing it.

Everything earlier in the check agreed with AWS: the denied run reached FAILED, glue.GetTable for the table answered EntityNotFoundException / "Entity Not Found", GetTables listed none, the driver stderr named AccessDeniedException and glue:CreateTable, the control run reached SUCCESS and its table carried PartitionKeys [("dt","string")]. Only the row count/sum read back over the table's data diverged.

## Steps to Reproduce
1. Create an S3 bucket; put a CSV of N rows (columns id,dt,amount over a few dt values) at input/facts.csv and the PySpark program at job/ddl.py. The program reads the CSV and does:
   facts.write.mode("overwrite").format("parquet").partitionBy("dt").option("path", s3://<bucket>/warehouse/<tbl>/).saveAsTable("<db>.<tbl>")
2. Create the Glue database <db> (and the `default` database the Hive metastore client bootstraps) and an Athena workgroup whose ResultConfiguration.OutputLocation is in the bucket.
3. Create two IAM execution roles that differ ONLY in glue:CreateTable: role A has every other Glue metastore action (GetDatabase, CreateDatabase, GetTable(s), UpdateTable, DeleteTable, Create/Batch/Get/Update/Delete partition) and S3 access; role B is role A plus glue:CreateTable.
4. emr-serverless CreateApplication (type SPARK, releaseLabel emr-7.9.0) with an s3MonitoringConfiguration logUri in the bucket.
5. StartJobRun with executionRoleArn = role A and a sparkSubmit jobDriver pointing at job/ddl.py (hive catalog implementation, AWSGlueDataCatalogHiveClientFactory). Poll GetJobRun to the terminal state: it ends FAILED, and glue.GetTable for <tbl> answers EntityNotFoundException — correct. Note that objects now exist under s3://<bucket>/warehouse/<tbl>/.
6. StartJobRun again on the same application with executionRoleArn = role B and the identical jobDriver. Poll GetJobRun: it ends SUCCESS and glue.GetTable now returns the table with PartitionKeys [("dt","string")].
7. Run the Athena query SELECT count(*) AS c, sum(amount) AS s FROM "<db>"."<tbl>" in that workgroup and read the single data row.
   Expected: (N, sum of the seeded amounts).
   Actual: (2N, 2x the seeded sum) — the failed run's output was never replaced by the overwrite write.
