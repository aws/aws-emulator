Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp__iam__logs.user-import-job-role-delivers-per-line-results-to-logs cognito-idp__iam__logs.restart-of-finished-import-job-rejected-and-logs-nothing-more
Issue-Key: cognito-idp__iam__logs.user-import-job-role-delivers-per-line-results-to-logs|run#37|Timeout|time.sleep(2)
Issue-Key: cognito-idp__iam__logs.restart-of-finished-import-job-rejected-and-logs-nothing-more|run#35|Timeout|time.sleep(2)
Issue-Commit: 7300602cd0fc37394a6328fdd869591f4a36c031

# cognito-idp: a user import job never reaches a terminal state — no per-line results are delivered to CloudWatch Logs and a finished job can never be tested for restart refusal

> _Note: this issue was identified against the code base at commit `7300602cd0fc37394a6328fdd869591f4a36c031`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartUserImportJob

## Description
Cause 1 of 1: a Cognito user-import job started with a CloudWatchLogsRoleArn never progresses to a terminal status and never writes its per-line results, so neither the delivery nor the post-completion refusal can be observed.

AWS: `StartUserImportJob` moves the job to `InProgress`, Cognito assumes the job's `CloudWatchLogsRoleArn` and writes one log event per CSV line into the job's log group (`/aws/cognito/<user-pool-id>/<job-id>`-style vended group), then the job settles in `Succeeded`/`Failed` with `ImportedUsers`/`FailedUsers` counts. A `StartUserImportJob` on a job that has already finished is refused with a 400 error and writes nothing further. Smithy (cognito-idp): `UserImportJobType.Status` is the enum Created|Pending|InProgress|Stopping|Expired|Stopped|Failed|Succeeded, and `StartUserImportJob` models `PreconditionNotMetException`/`ResourceNotFoundException`/`NotAuthorizedException`; the log delivery is documented behavior only.

Emulator: both checks end `Check timed out after 60.0s` — the job never settles, nothing is delivered to the log group, and the restart case can never be exercised because no job ever becomes 'finished'.

Why: a client that imports users cannot tell whether the import ran, which rows failed, or why — the per-line log is the only record of row-level outcomes. Code that polls DescribeUserImportJob for a terminal Status hangs, and the documented refusal of a restart is unobservable, so error handling around it is never exercised.

Fix: the user-import job runner — process the uploaded CSV, deliver per-line results through the job's CloudWatchLogsRoleArn, settle the job in a terminal Status, and refuse StartUserImportJob for a job already in a terminal Status.

Traces: cognito-idp__iam__logs.user-import-job-role-delivers-per-line-results-to-logs, cognito-idp__iam__logs.restart-of-finished-import-job-rejected-and-logs-nothing-more.

## Evidence
Both checks: "Error: Check timed out after 60.0s". Expected: the job reaches a terminal Status, one log event per CSV line appears in the job's log group, and a second StartUserImportJob on the finished job is refused with a 400 error and writes nothing more.

## Steps to Reproduce
1. Create a user pool, a CloudWatch Logs role, and a user import job (CreateUserImportJob with CloudWatchLogsRoleArn); upload a small CSV to the job's PreSignedUrl.
2. StartUserImportJob, then poll DescribeUserImportJob — the Status never reaches a terminal value and the job's log group stays empty.
3. Attempt StartUserImportJob again — cannot be exercised, because the job never finishes.
