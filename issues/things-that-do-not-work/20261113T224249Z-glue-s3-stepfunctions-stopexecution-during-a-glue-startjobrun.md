Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue__s3__stepfunctions.xsvc-sfn-1-glue-sync-stop-execution-stops-the-running-job-run
Issue-Key: glue__s3__stepfunctions.xsvc-sfn-1-glue-sync-stop-execution-stops-the-running-job-run|_wait_run_state#12|Timeout|state = glue.get_job_run(JobName=job, RunId=run_id)["JobRun"]["JobRunState"]
Issue-Commit: 23d7c3ca3e68cb70ca93342274fc19d1394baa26

# glue__s3__stepfunctions: StopExecution during a glue:startJobRun.sync wait never reaches a terminal Glue job run — the check makes no progress and is killed at its 60s budget

> _Note: this issue was identified against the code base at commit `23d7c3ca3e68cb70ca93342274fc19d1394baa26`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StopExecution

## Description
One cause.

Cause 1 of 1: the glue:startJobRun.sync cancellation path never completes — StopExecution on an execution parked in a glue:startJobRun.sync wait does not drive the downstream Glue job run to a terminal state (and/or the submitted run never becomes observable as RUNNING), so the check's bounded polls never satisfy their condition and the check is killed at its 60s budget with no assertion ever reached. Failing check id: glue__s3__stepfunctions.xsvc-sfn-1-glue-sync-stop-execution-stops-the-running-job-run.

AWS: for the optimized `.sync` integration `arn:aws:states:::glue:startJobRun.sync`, Step Functions submits glue:StartJobRun and then polls the run to completion. StopExecution(executionArn, error, cause) on an execution parked in that wait cancels the downstream work: the Glue job run reaches JobRunState STOPPED (Glue GetJobRun(JobName, RunId) reports it), the job's side effect never lands (no marker object in S3), the execution becomes ABORTED carrying exactly the stopper's error and cause and no output, and GetExecutionHistory ends with ExecutionAborted containing neither TaskSucceeded nor TaskFailed — a task stopped mid-wait neither succeeds nor fails. Smithy (sfn) models StopExecution with executionArn @required plus error/cause and the client errors ExecutionDoesNotExist, InvalidArn, ValidationException and the KMS errors; the cancellation propagation itself is not expressible in the model, so this is recorded behavior only, as asserted by the check.

Emulator: the check never reaches any of those assertions. Its verdict is `mismatch` with the sole detail `Check timed out after 60.0s` — i.e. within the harness budget one of the bounded waits (the TaskSubmitted/run-id wait after StartExecution, the wait for the run to be observable as RUNNING, or the wait for the run to go terminal after StopExecution) never saw its condition. Hypothesis: the .sync cancellation leg is missing, so after StopExecution the submitted Glue run stays non-terminal forever and the post-stop wait spins until the budget expires. The emulator is the stalling party: the same 60s-budget symptom is what 32 other failing checks in this run report, and sibling glue .sync checks in this service show the .sync completion-poll path mishandling non-success outcomes (already filed as 20261110T193514Z-glue-s3-stepfunctions-a-glue-startjobrun-sync-task.md, which covers only the denied-GetJobRun termination and does not carry this check's replay key).

Why: a client that cancels a long-running ETL step by stopping the execution relies on the Glue run actually stopping. If it does not, the job keeps running and its side effects still land after the cancel (here: the marker object would be written to S3 despite the abort), spend continues on a run nobody is waiting for, and the execution's ABORTED record no longer describes the world. Worse, any caller that waits for the downstream run to go terminal after StopExecution — the documented way to confirm a cancel — never returns.

Fix: the Step Functions `.sync` service-integration waiter for glue:startJobRun.sync, specifically its cancellation hook: StopExecution (and any abort of an execution parked in a .sync wait) must call the integration's stop action for the submitted run (Glue BatchStopJobRun for the recorded JobName/RunId) and let the run settle to JobRunState STOPPED, while terminating the task without emitting TaskSucceeded or TaskFailed and ending the execution ABORTED with the stopper's error and cause. The submit leg must also make the run observable (TaskSubmitted carrying JobName/RunId, GetJobRun reporting RUNNING) promptly so a cancel has something to target.

Traces: glue__s3__stepfunctions.xsvc-sfn-1-glue-sync-stop-execution-stops-the-running-job-run.

## Evidence
Check glue__s3__stepfunctions.xsvc-sfn-1-glue-sync-stop-execution-stops-the-running-job-run, verdict mismatch:

    Error: Check timed out after 60.0s

No assertion was reached and no exception was raised by the check body — the emulator simply never advanced the state the check polls for, so the harness killed the run at its budget.

What the check does and expects (real AWS):
1. StartExecution of a single-Task state machine whose Resource is `arn:aws:states:::glue:startJobRun.sync`, with the job in "sleep" mode; poll the history until TaskSubmitted yields the Glue RunId.
2. Glue GetJobRun(JobName, RunId).JobRun.JobRunState becomes RUNNING, and DescribeExecution is RUNNING at that moment.
3. StopExecution(executionArn, error="sim2real.Stop", cause="stop while parked").
4. Expected afterwards: GetJobRun reports JobRunState == "STOPPED"; the job's output object s3://<bucket>/out/<token>.txt does NOT exist; DescribeExecution is status ABORTED with error/cause exactly as passed to StopExecution; GetExecutionHistory's last event type is "ExecutionAborted" and the history contains neither "TaskSucceeded" nor "TaskFailed".
Actual (emulator): none of this is observable — one of the bounded polls above never satisfies its predicate and the check is terminated at 60.0s.

The error string "ABORTED", the JobRunState "STOPPED" and the event names "ExecutionAborted"/"TaskSucceeded"/"TaskFailed" are fixed text; the bucket name, object key, job name, RunId, execution ARN and the stopper's error/cause are values taken from the request and state.

## Steps to Reproduce
1. Create an S3 bucket and upload a Python-shell script that sleeps for 600s when invoked with `--mode sleep`, and otherwise writes a marker object to `out/<token>.txt`.
2. Create a Glue Python-shell job (pythonshell, MaxCapacity 0.0625, MaxConcurrentRuns 4) pointing at that script, on a glue.amazonaws.com-trusted role allowed s3:GetObject on `script/*` and s3:PutObject on `out/*`.
3. Create a states.amazonaws.com-trusted execution role allowed glue:StartJobRun, glue:GetJobRun, glue:GetJobRuns and glue:BatchStopJobRun on that job ARN.
4. Create a STANDARD state machine with a single Task state, Resource `arn:aws:states:::glue:startJobRun.sync`, Parameters naming the job and passing `--mode sleep`.
5. StartExecution with input {"mode":"sleep","key":"out/<token>.txt","token":"<token>"} and read the RunId from the TaskSubmitted history event.
6. Poll Glue GetJobRun(JobName, RunId) until JobRunState == RUNNING; confirm DescribeExecution is RUNNING.
7. Call StopExecution(executionArn, error="sim2real.Stop", cause="stop while parked").
8. Poll GetJobRun for a terminal JobRunState.
   -> AWS: STOPPED within the wait; no marker object in S3; DescribeExecution ABORTED with the given error/cause; history ends with ExecutionAborted and has no TaskSucceeded/TaskFailed.
   -> Emulator: the polls never reach their condition and the check is killed at its 60s budget; the downstream Glue run is never driven to a terminal state by the stop.
