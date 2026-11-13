Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena__emr-serverless__glue__s3.spark-saveastable-without-glue-createtable-permission-is-denied
Issue-Key: athena__emr-serverless__glue__s3.spark-saveastable-without-glue-createtable-permission-is-denied|_await_driver_stderr#31|Timeout|time.sleep(15)
Issue-Commit: 56c908e9384776713583471da50d0f78dde9fb40

# EMR Serverless publishes no job-run logs to the configured s3MonitoringConfiguration.logUri, so a client waiting for SPARK_DRIVER/stderr.gz never terminates

> _Note: this issue was identified against the code base at commit `56c908e9384776713583471da50d0f78dde9fb40`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartJobRun

## Description
One cause.

Attribution note: the failing check's id leads with the four-service combo token `athena__emr-serverless__glue__s3`, which equals no token in get_bootstrap_status (neither bootstrapped nor pending), so the per-service gate cannot place it as written. The divergence described here is produced by the emr-serverless log-publishing path (the application's monitoringConfiguration.s3MonitoringConfiguration.logUri supplied to CreateApplication, and the job run started by StartJobRun), and emr-serverless IS stood up and answering — it passes 34/34 of its own behavioral checks in this same replay — so the issue is filed against `emr-serverless`. It is NOT attributed to `athena` (which is pending bootstrap): the check timed out in its log wait, before any Athena call in the script.

Related but not covering: open issue 20261024T013257Z-emr-serverless-job-runs-never-execute-the-submitted.md carries the OTHER check of this group (spark-created-glue-table-read-back-by-athena) and its cause is the terminal state being FAILED instead of SUCCESS. It does not carry this check's replay key, and this cause is observable independently of the terminal state: in AWS, a job run that reaches FAILED still publishes its driver log. This issue is only about the absent log objects, not about the terminal state.

Cause 1 of 1: a job run started on an application whose monitoringConfiguration.s3MonitoringConfiguration.logUri points into an S3 bucket never causes any log object to appear under that prefix — in particular no `<logUri>applications/<applicationId>/jobs/<jobRunId>/SPARK_DRIVER/stderr.gz`.

AWS: Smithy com.amazonaws.emrserverless#S3MonitoringConfiguration.logUri is documented as "The Amazon S3 destination URI for log publishing", and #MonitoringConfiguration.s3MonitoringConfiguration as "The Amazon S3 configuration for monitoring log publishing" with the note "You can configure your jobs to send log information to Amazon S3." #JobRun is "a unit of work, such as a Spark JAR, Hive query, or SparkSQL query, that you submit to an Amazon EMR Serverless application" and its required `state`/`stateDetails` reach a terminal value once the run ends. With that configuration set on the application, a SPARK job run publishes its per-worker-type logs under the logUri, keyed by application id and job run id, with the driver's stderr written as a gzip stream; this happens for a run that ended FAILED as well as one that ended SUCCESS, which is exactly how a failure's cause is diagnosed. The exact key layout under the logUri (applications/<applicationId>/jobs/<jobRunId>/SPARK_DRIVER/stderr.gz) is recorded behavior only — the Smithy model does not state the key scheme — but the existence of SOME published log content under the configured logUri after a job run terminates is the modelled contract of that member.
Emulator: no object at all appears under the job run's log prefix. The suite reports this check as `[mismatch] ... Error: Check timed out after 60.0s` with no assertion text, because the divergence is an absence: the check's `_await_driver_stderr` helper lists keys under `<logUri>applications/<app_id>/jobs/<job_run_id>/` and sleeps 15 s between polls on a 420 s budget, so it can only ever exit by timing out. That the emulator never produces the run's side effects is stated by the emulator itself in the companion check's stateDetails ('This emulator records job runs but executes no Spark or Hive workload, so the job run ended without running.'), and the already-filed companion issue records the same absence in its reproduce steps. The preceding calls of this check are known to complete promptly against this emulator: the companion check in the same group performs the same CreateApplication/StartJobRun/GetJobRun sequence and reaches its own terminal-state assertion well inside the same 60 s wrapper, so the unbounded wait is the log wait, not the control-plane polling.
Why: every client that uses EMR Serverless logs to diagnose a run breaks, and it breaks by HANGING rather than by erroring. The AWS-documented pattern after a terminal job run is to fetch the driver log from the configured logUri; against the emulator that read finds nothing, so a poller spins to its own deadline (here the check's 420 s budget, in a pipeline whatever the orchestrator allows) and CI time is burned on a wait that can never succeed. A caller that instead reads once gets an empty listing and concludes the run produced no diagnostics, which in AWS would mean a platform fault. It also makes the IAM-denial scenario ungradable end to end: the only place the denied `glue:CreateTable` action is visible is the AccessDeniedException the driver wrote into its own stderr, and with no published stderr that evidence does not exist anywhere in the emulator.
Fix: the emr-serverless job-run lifecycle and its monitoring/log-publication path — the component that handles StartJobRun and drives the run to a terminal state must also honour the application's (and the run's configurationOverrides') monitoringConfiguration.s3MonitoringConfiguration by writing the run's log artifacts into the configured S3 logUri under the per-application/per-job-run prefix, including a gzipped SPARK_DRIVER/stderr for a run that terminated, so the artifacts exist for both a successful and a failed run. Modelling it alongside the state machine is what makes the terminal state diagnosable.
Traces: athena__emr-serverless__glue__s3.spark-saveastable-without-glue-createtable-permission-is-denied (timed out in `_await_driver_stderr`, which waits for `<logUri>applications/<app_id>/jobs/<job_run_id>/SPARK_DRIVER/stderr.gz`).

## Evidence
Replay of simparity-behavioral-ga-prod-v1: 34/36 passed, 2 mismatched.

[mismatch] athena__emr-serverless__glue__s3.spark-saveastable-without-glue-createtable-permission-is-denied
  Error: Check timed out after 60.0s

The check (check_athena__emr_serverless__glue__s3_spark_saveastable_without_glue_createtable_permission_is_denied.py) creates a bucket, an IAM execution role, a Glue database, an Athena workgroup and an EMR Serverless application with
  monitoringConfiguration={"s3MonitoringConfiguration": {"logUri": "s3://<bucket>/logs/"}}
starts a sparkSubmit job run, waits for a terminal state, and then calls:

    def _await_driver_stderr(s3, bucket, log_prefix, app_id, job_run_id, budget_s):
        want = f"{log_prefix}applications/{app_id}/jobs/{job_run_id}/SPARK_DRIVER/stderr.gz"
        deadline = time.monotonic() + budget_s          # budget_s = _LOG_BUDGET_S = 420.0
        while time.monotonic() < deadline:
            seen = _list_keys(s3, bucket, f"{log_prefix}applications/{app_id}/jobs/{job_run_id}/")
            if want in seen: ...
            time.sleep(15)
        raise AssertionError(f"no non-empty {want!r} appeared within {budget_s}s; ...")

Expected (AWS): after the job run terminates, ListObjectsV2 under
  logs/applications/<applicationId>/jobs/<jobRunId>/
returns the run's published log objects, including a non-empty gzip object
  logs/applications/<applicationId>/jobs/<jobRunId>/SPARK_DRIVER/stderr.gz
whose decompressed text is the driver's stderr (for this scenario, naming AccessDeniedException and the denied glue:CreateTable action). This holds for a run whose terminal state is FAILED as well as SUCCESS.

Actual (emulator): that prefix stays empty for the life of the run, so the wait cannot exit and the check is killed by the harness wrapper at 60 s with no assertion reached. CreateApplication accepted the monitoringConfiguration and StartJobRun returned 200 with a jobRunId, so nothing in any control-plane response signals that no logs will ever be published. The emulator's own stateDetails for job runs ('This emulator records job runs but executes no Spark or Hive workload, so the job run ended without running.') states that the run produces no workload side effects; the absent log object is one of them.

Note on scope: the same check's later steps (the positive control run that must reach SUCCESS, the Glue PartitionKeys assertion, and the Athena read-back) were never reached. The SUCCESS half is the cause already filed as 20261024T013257Z-emr-serverless-job-runs-never-execute-the-submitted.md and is deliberately not re-filed here.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-behavioral-ga-prod-v1 behavioral suite; check athena__emr-serverless__glue__s3.spark-saveastable-without-glue-createtable-permission-is-denied times out.
2. By hand, with the emulator endpoint:
   a. s3 CreateBucket <bucket>; PutObject <bucket>/input/facts.csv (a header'd CSV id,dt,amount) and <bucket>/job/ddl.py (a PySpark script doing spark.read.csv(...).write.partitionBy('dt').option('path', warehouse).saveAsTable(db + '.' + tbl)).
   b. glue CreateDatabase for 'default' and for <db>.
   c. iam CreateRole trusted by emr-serverless.amazonaws.com + PutRolePolicy granting the S3 object/bucket actions on <bucket> and the glue table/partition actions.
   d. emr-serverless CreateApplication with releaseLabel 'emr-7.9.0', type 'SPARK', and monitoringConfiguration={'s3MonitoringConfiguration': {'logUri': 's3://<bucket>/logs/'}} -> note the returned applicationId.
   e. emr-serverless StartJobRun with jobDriver.sparkSubmit.entryPoint = s3://<bucket>/job/ddl.py, entryPointArguments = [s3://<bucket>/input/facts.csv, <db>, <tbl>, s3://<bucket>/warehouse/<tbl>/], and sparkSubmitParameters setting spark.sql.catalogImplementation=hive plus the AWSGlueDataCatalogHiveClientFactory metastore factory -> note the returned jobRunId.
   f. Poll emr-serverless GetJobRun until jobRun.state is terminal (SUCCESS / FAILED / CANCELLED).
3. s3 ListObjectsV2 --bucket <bucket> --prefix logs/applications/<applicationId>/jobs/<jobRunId>/ , repeatedly for several minutes.
4. Observe the listing stays empty: no SPARK_DRIVER/stderr.gz (and no log object at all) ever appears under the configured logUri, so any client polling for the driver log runs to its own deadline. In AWS the prefix is populated once the run terminates, and GetObject on .../SPARK_DRIVER/stderr.gz returns a non-empty gzip stream containing the driver's stderr.
