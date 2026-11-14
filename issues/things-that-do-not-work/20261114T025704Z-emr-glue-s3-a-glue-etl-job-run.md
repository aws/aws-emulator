Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr__glue__s3.glue-job-missing-s3-getobject-source-rejected
Issue-Key: emr__glue__s3.glue-job-missing-s3-getobject-source-rejected|_wait_for_job_run#11|Timeout|time.sleep(2)
Issue-Commit: d4edb10874cfe8cae1b981ecc37a41d1433fca3c

# emr__glue__s3: a Glue ETL job run never reaches a terminal state, so neither the denied-source FAILED run nor the permitted positive control can be observed

> _Note: this issue was identified against the code base at commit `d4edb10874cfe8cae1b981ecc37a41d1433fca3c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetJobRun

## Description
Cause 1 of 1: StartJobRun is accepted but the run never settles — GetJobRun never reports a JobRunState in {SUCCEEDED, FAILED, TIMEOUT, ERROR, STOPPED} — so the check exhausts the harness's 60 s budget while polling, and both halves of the permission boundary (a role denied s3:GetObject on the source prefix must FAIL the run and write no output; a role with that grant must SUCCEED and land a part- object) are unreachable.

AWS: glue:StartJobRun starts a Glue ETL run; the run executes and settles in a terminal JobRunState that GetJobRun reports, with ErrorMessage set on failure. A role that cannot read the source prefix yields a FAILED run whose ErrorMessage shows the denied read (Hadoop's S3A masks the 403 as a missing path during schema inference) and no output objects; a role with s3:GetObject on the source yields SUCCEEDED and at least one "part-" object under the output prefix. Smithy: glue#StartJobRun returns StartJobRunResponse{JobRunId}; glue#GetJobRun returns JobRun{JobRunState, ErrorMessage, ...} whose JobRunState enum carries those terminal values.
Emulator: "Error: Check timed out after 60.0s" while polling GetJobRun for a terminal state; the run stays non-terminal, so neither the FAILED verdict and its ErrorMessage nor the successful control's output object exist.
Why: a client cannot tell a job that failed from a job that is still running, cannot read a failure reason, and cannot use run outcome to gate downstream steps; an IAM misconfiguration on the job role produces the same observation as a healthy job.
Fix: the Glue job-run executor and the StartJobRun/GetJobRun run-state machine, including terminal-state ErrorMessage population for a run whose source read is refused.
Traces: emr__glue__s3.glue-job-missing-s3-getobject-source-rejected.

Open issues record the same engine gap under other service groups (for example 20261029T175551Z-glue-job-runs-reached-through-the-emr-glue.md and 20261025T000830Z-glue-job-runs-are-recorded-but-never-executed.md); neither carries this check's replay key.

## Evidence
emr__glue__s3.glue-job-missing-s3-getobject-source-rejected — Error: Check timed out after 60.0s.
Expected per AWS: GetJobRun(JobName, RunId) reports JobRunState "FAILED" with an ErrorMessage naming the unreadable source path for the denied role (and no objects under the output prefix), and "SUCCEEDED" with at least one part- object for the role that holds s3:GetObject on the source prefix.
Actual: the run never leaves its non-terminal state within the budget, so `assert bad_run["JobRunState"] == "FAILED"` is never reached.

## Steps to Reproduce
1. Create a bucket holding data/input.csv and scripts/read_csv.py.
2. Create two Glue ETL jobs on the same script with two roles: one denied s3:GetObject on data/*, one granted it.
3. StartJobRun each and poll GetJobRun for a terminal state: AWS settles FAILED (no output) and SUCCEEDED (one part- object counting 3 rows); the emulator never settles either run.
