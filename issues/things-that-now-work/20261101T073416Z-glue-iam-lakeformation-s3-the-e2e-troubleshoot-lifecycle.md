Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue__iam__lakeformation__s3.e2e-troubleshoot-glue-job-table-read-failure-lifecycle
Issue-Key: glue__iam__lakeformation__s3.e2e-troubleshoot-glue-job-table-read-failure-lifecycle|phase#6|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Commit: 151c22cb4b43a38a7601f8301334a881571db6d4

# glue__iam__lakeformation__s3: the e2e troubleshoot lifecycle's post-grant Glue job run still ends FAILED without executing the script (1 cause)

> _Note: this issue was identified against the code base at commit `151c22cb4b43a38a7601f8301334a881571db6d4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartJobRun

## Description
Cause 1 of 1: a Glue job run is recorded but never executed, so the run that must SUCCEED after the Lake Formation table grant terminates FAILED with an emulator-authored ErrorMessage, and the whole troubleshoot-and-fix lifecycle can never reach its repaired state.

AWS: Smithy (services/service-com-amazonaws-glue-awsglue/smithy-model/com-amazonaws-glue-awsglue.json) models StartJobRun -> StartJobRunResponse.JobRunId and GetJobRun -> JobRun, whose JobRunState enum carries STARTING/RUNNING/SUCCEEDED/FAILED/...; the model declares no 'not executed' outcome. Real Glue runs the job's script under the job's Role: a run whose role is authorized for the governed catalog table and its S3 location transitions to SUCCEEDED, carries NO ErrorMessage, and leaves the script's side effects behind (here the row count the ETL script writes back to the stack bucket). A run whose role lacks the Lake Formation grant fails with an authorization ErrorMessage, so the FAILED->SUCCEEDED flip is driven solely by the lakeformation GrantPermissions write. Beyond the state enum this is recorded behavior only.

Emulator: in the `solve` phase of the e2e lifecycle, after GrantPermissions(SELECT, DESCRIBE) on the table to the job's role, StartJobRun is accepted and GetJobRun polls to the terminal state 'FAILED' with ErrorMessage 'This emulator records job runs but executes no job script, so the job run ended without running.' The run outcome is therefore independent of both the role's IAM policies and the Lake Formation grant, no script side effect exists, and the subsequent s3 GetObject of the run's output has nothing to read (fixed_count is never derived). Of that message, the whole text is emulator-authored: AWS has no such fixed text and no such variable parts; a FAILED Glue run's ErrorMessage states the run's actual failure (for this scenario, the Lake Formation / catalog authorization failure naming the database and table from the request and the role from the job definition), and a SUCCEEDED run carries no ErrorMessage member at all.

Why: a client cannot use this emulator to validate or troubleshoot a Lake-Formation-governed ETL job. The pre-fix FAILED and the post-fix FAILED are identical, so the grant that repairs the job is unobservable, a correct remediation looks like a still-broken job, and automation that gates on JobRunState == 'SUCCEEDED' (or on the job's output objects) never converges.

Fix: the StartJobRun/GetJobRun run lifecycle in the glue service crate's job-run state machine — a started run must advance to SUCCEEDED when the job role is authorized for the catalog resources and S3 location the script reads (performing the script's modelled side effects), and to FAILED with the real authorization error when it is not, rather than settling to a fixed 'executes no job script' failure.

Related already-open issues, named for the fixer and NOT a substitute for this one: 20261025T000319Z-glue-iam-lakeformation-s3-a-glue-job-run.md owns the same cause for the two sibling single-scenario checks (glue-job-catalog-read-denied-until-lf-grant, glue-job-denied-on-governed-table-without-lf-data-admin-until-lf-grant) and 20261023T123230Z-glue-startjobrun-is-an-unimplemented-501-scaffold-and.md covers StartJobRun answering 501 under the plain `glue` token; the replay report attributed neither to this e2e lifecycle check, whose replay key is not carried by either.

Traces: glue__iam__lakeformation__s3.e2e-troubleshoot-glue-job-table-read-failure-lifecycle (phase 3/5 `solve`).

## Evidence
Failing check glue__iam__lakeformation__s3.e2e-troubleshoot-glue-job-table-read-failure-lifecycle [mismatch]:

AssertionError: [phase 3/5 solve] after the grant the job ended 'FAILED': 'This emulator records job runs but executes no job script, so the job run ended without running.'

Expected (AWS): after lakeformation GrantPermissions(SELECT, DESCRIBE) on the governed table to the job's role, GetJobRun().JobRun.JobRunState == 'SUCCEEDED' with no ErrorMessage member, and the script's output object present in the stack bucket so the row count of the seeded CSV can be read back via s3 GetObject.
Actual (emulator): JobRunState == 'FAILED' with ErrorMessage 'This emulator records job runs but executes no job script, so the job run ended without running.' — the same terminal outcome as the pre-grant run, no output object written, so the lifecycle's required findings fixed_run_state/fixed_count are never established.

The earlier phases of the check pass: the CloudFormation stack deploys, the database reports CreateTableDefaultPermissions == [], IAM_ALLOWED_PRINCIPALS is revoked on database and table, GetJob/GetJobRuns/GetDatabase/ListPermissions/ListAttachedRolePolicies all answer, and the pre-grant run is FAILED. Only the post-grant run's execution diverges.

## Steps to Reproduce
1. Build and boot the emulator from this checkout, then replay the behavioral suite.
2. Deploy the governed-table stack (S3 bucket + Glue database with empty CreateTableDefaultPermissions + CSV table + Glue job + job role with AWSGlueServiceRole, AmazonS3FullAccess, AWSLakeFormationDataAdmin).
3. Upload the ETL script and a seed CSV to the bucket; lakeformation GrantPermissions ALL to IAM_ALLOWED_PRINCIPALS then RevokePermissions on both the database and the table so both are governed.
4. glue StartJobRun on the job and poll GetJobRun to a terminal state — it is FAILED (expected, but for the wrong reason).
5. lakeformation GrantPermissions(SELECT, DESCRIBE) on the table to the job's role, then StartJobRun again and poll GetJobRun: observe JobRunState 'FAILED' with ErrorMessage 'This emulator records job runs but executes no job script, so the job run ended without running.' instead of 'SUCCEEDED' with no ErrorMessage, and no output object under the bucket for s3 GetObject.
6. Re-replay the check id in trace_ids to confirm the fix.
