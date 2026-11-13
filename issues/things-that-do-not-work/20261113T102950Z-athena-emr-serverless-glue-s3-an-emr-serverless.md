Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena__emr-serverless__glue__s3.spark-saveastable-without-glue-createtable-permission-is-denied
Issue-Key: athena__emr-serverless__glue__s3.spark-saveastable-without-glue-createtable-permission-is-denied|run#31|AssertionError|assert "AccessDeniedException" in stderr_text, (
Issue-Commit: a92bf394ff6ba595d0d96d1f253c81da2b113daf

# athena__emr-serverless__glue__s3: an EMR Serverless Spark run never executes the submitted workload, so a Glue-permission refusal never reaches the driver log — SPARK_DRIVER/stderr names the emulator's own "executes no Spark or Hive workload" message instead of AccessDeniedException

> _Note: this issue was identified against the code base at commit `a92bf394ff6ba595d0d96d1f253c81da2b113daf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartJobRun

## Description
One net-new root cause, grounded in a failing behavioral check of the `athena__emr-serverless__glue__s3` suite.

=== Cause 1 of 1: a submitted EMR Serverless job run is recorded and driven straight to FAILED without running the Spark application, so no failure the WORKLOAD would raise is ever observable in its driver log ===

AWS: `StartJobRun` submits a job run that the application actually executes, and the run's driver output is delivered to the configured `s3MonitoringConfiguration.logUri` under the SPARK_DRIVER/stderr.gz log stream. Smithy (com.amazonaws.emrserverless#AwsToledoWebService): `StartJobRun` takes `jobDriver` (a union whose `sparkSubmit` member carries `entryPoint`/`sparkSubmitParameters`), `executionRoleArn` "The execution role ARN for the job run", and `configurationOverrides.monitoringConfiguration.s3MonitoringConfiguration.logUri`; `GetJobRun` reports `JobRun$state` over the `JobRunState` set (SUBMITTED, PENDING, SCHEDULED, RUNNING, SUCCESS, FAILED, CANCELLING, CANCELLED, QUEUED) with `stateDetails`. The model says nothing about driver-log CONTENT — that part is recorded behavior: a Spark job that calls `saveAsTable` through the Glue metastore client while its execution role is missing `glue:CreateTable` fails inside the driver, and the driver log names the Glue refusal class `AccessDeniedException` (the Glue control-plane refusal the metastore client surfaces), with every other Glue read/write action still granted so the removed action is the only difference.

Emulator: the run is accepted, walks SUBMITTED → PENDING → QUEUED → SCHEDULED → FAILED on a synthesized clock, and the only bytes delivered to the driver log are the emulator's own status narration, ending with "ERROR Job run 0000000000000001 is FAILED. This emulator records job runs but executes no Spark or Hive workload, so the job run ended without running." No Spark application runs, so the Glue metastore call that the check removed a permission from is never made, and `AccessDeniedException` appears nowhere in the driver log. The run also reports a canned job-run id and a canned submitter/execution-role narration rather than workload output.

Why: a client relying on AWS's behavior reads the driver log to find out WHY a run failed and to distinguish failure classes — an authorization refusal from the catalog, a schema error, a data error, an OOM. Against the emulator every run fails for the same emulator-authored reason, so (a) a genuine permission misconfiguration is indistinguishable from a healthy pipeline that merely "ended without running", (b) a negative test that asserts a denial is unprovable, and (c) CI that greps the driver log for a failure class gets the emulator's prose instead.

Fix: the EMR Serverless job-run executor behind `StartJobRun` — services/service-com-amazonaws-emrserverless-awstoledowebservice/src/ops/start_job_run.rs and the job-run state machine plus log writer it drives in that crate's src/state.rs. The run must execute the submitted `sparkSubmit` workload (or at least evaluate the catalog/storage calls the workload makes under the run's `executionRoleArn`), reach SUCCESS or FAILED from that outcome, and deliver the workload's own driver output — including a Glue `AccessDeniedException` when the execution role lacks `glue:CreateTable` — to `s3MonitoringConfiguration.logUri`.

Traces: athena__emr-serverless__glue__s3.spark-saveastable-without-glue-createtable-permission-is-denied.

## Evidence
Failing check: athena__emr-serverless__glue__s3.spark-saveastable-without-glue-createtable-permission-is-denied — verdict `mismatch`.

Assertion, from the replay report:
  check_athena__emr_serverless__glue__s3_spark_saveastable_without_glue_createtable_permission_is_denied.py, line 438, in run
    assert "AccessDeniedException" in stderr_text, (
  AssertionError: the driver log for the denied run does not name AccessDeniedException (tail: '…INFO Submitted by arn:aws:iam::<account>:user/EMULATOR… under execution role arn:aws:iam::<account>:role/sim2real-emrs-deny-role-…\n… INFO Job run <id> is SUBMITTED. Job run submitted.\n… INFO Job run <id> is PENDING. Job run accepted and pending admission.\n… INFO Job run <id> is QUEUED. Job run queued, waiting for capacity.\n… INFO Job run <id> is SCHEDULED. Job run scheduled on the application.\n… ERROR Job run <id> is FAILED. This emulator records job runs but executes no Spark or Hive workload, so the job run ended without running.\n')

Expected: the SPARK_DRIVER/stderr log of the run whose execution role is granted every enumerated Glue read/write action EXCEPT `glue:CreateTable` names `AccessDeniedException` — the Glue refusal the Spark metastore client raises when `saveAsTable` tries to create the table.
Actual: the driver log contains only synthesized EMR-Serverless status lines and terminates with the emulator's own sentence "This emulator records job runs but executes no Spark or Hive workload, so the job run ended without running."

The account id, role name, job-run id and timestamps in that tail are per-run values of this recording and not part of the requirement; the requirement is that the driver log carries the WORKLOAD's failure, naming the Glue `AccessDeniedException` class.

Related open issues that do NOT carry this check's replay key: 20261024T013257Z-emr-serverless-job-runs-never-execute-the-submitted.md and 20261025T015858Z-emr-serverless-publishes-no-job-run-logs-to.md describe the same executor gap from the `emr-serverless` suite; 20261030T093552Z-emr-serverless-job-run-lifecycle-diverges-runs-never.md covers the state walk. None of them lists this check, which is why it is filed here — it is the leg that shows the gap also destroys a NEGATIVE (authorization) verdict, not just the happy path.

## Steps to Reproduce
1. Create an S3 bucket, a Glue database, and an EMR Serverless application (release emr-7.9.0).
2. Create an execution role granting every Glue catalog action the Spark metastore client needs to write a partitioned table — GetDatabase, GetDatabases, CreateDatabase, GetTable, GetTables, UpdateTable, DeleteTable, CreatePartition, BatchCreatePartition, GetPartition(s), UpdatePartition, BatchGetPartition, DeletePartition — but NOT `glue:CreateTable`, plus the S3 access the job needs.
3. `StartJobRun` a sparkSubmit job that writes a partitioned table with `saveAsTable` into that Glue database, with `configurationOverrides.monitoringConfiguration.s3MonitoringConfiguration.logUri` pointing at the bucket.
4. Poll `GetJobRun` to a terminal state, then fetch the run's SPARK_DRIVER/stderr.gz object under that logUri and decompress it.
5. Expect: the run is FAILED and the driver log names `AccessDeniedException` for the Glue CreateTable refusal.
   Observe: the run is FAILED with no workload output at all; the driver log ends with "This emulator records job runs but executes no Spark or Hive workload, so the job run ended without running."
6. Positive control (same shape, grant `glue:CreateTable` back): AWS runs the job to SUCCESS and the table becomes readable through Athena; the emulator fails it identically, which is the same cause.
