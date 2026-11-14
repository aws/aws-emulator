Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue__sqs.cli-glue-job-run-status-poll-sqs-completion-positive
Issue-Key: glue__sqs.cli-glue-job-run-status-poll-sqs-completion-positive|run#10|AssertionError|assert observed_running, "never observed the job in RUNNING state"
Issue-Commit: 2fe7204e4697cf4de9a1d25ac1d449d622f376b5

# glue__sqs: a Glue job run is never observable in RUNNING state — a polling client only ever sees the run before and after execution

> _Note: this issue was identified against the code base at commit `2fe7204e4697cf4de9a1d25ac1d449d622f376b5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetJobRun

## Description
Cause 1 of 1: GetJobRun never reports JobRunState RUNNING for a started job run, so a client polling a run through its lifecycle never observes the in-flight phase.

AWS: Smithy (com.amazonaws.glue) models JobRunState as the enum {STARTING, RUNNING, STOPPING, STOPPED, SUCCEEDED, FAILED, TIMEOUT, ERROR, WAITING, EXPIRED}. A run started by StartJobRun is reported by GetJobRun/GetJobRuns as STARTING, then RUNNING for as long as the script executes, then a terminal state (SUCCEEDED here). A pythonshell script that sleeps for 25 seconds before publishing therefore leaves a wide window in which any poller — including `aws glue get-job-run` — reads JobRunState == RUNNING.

Emulator: polling the run from StartJobRun through to its terminal state never yields RUNNING; the run is observed only in its pre-execution state and then in a terminal state. The check's first failing assertion is on that observation ("never observed the job in RUNNING state"), which means the poll loop did reach a terminal state without RUNNING ever being reported — the run has no observable execution phase.

Why: a client cannot distinguish a job that is executing from one that has not started or has already finished. Progress reporting, concurrency accounting and any wait loop keyed on RUNNING (the documented in-flight state) never trigger, and a run that AWS shows as in-flight for the duration of its script appears instantaneous. Combined with the script's own side effect (the SQS completion message) never arriving, the whole poll-then-confirm workflow reports nothing a real run reports.

Fix: the Glue job-run state machine behind StartJobRun/GetJobRun — the run lifecycle that must publish STARTING, then RUNNING while the script executes, then the terminal state, rather than jumping from the initial state to a terminal one.

Note: open issue 20261107T185637Z-glue-s3-stepfunctions-a-glue-job-run-reached-through-a-step.md records the same missing RUNNING phase reached through a Step Functions sync task, and 20261110T132412Z-glue-sqs-a-cloudformation-deployed-eventbridge-rule-never.md covers the absent state-change event; the suite reports this check as owned by neither.

## Evidence
Check glue__sqs.cli-glue-job-run-status-poll-sqs-completion-positive.
Expected (AWS): while polling `aws glue get-job-run` for a pythonshell run whose script sleeps 25s, at least one poll returns JobRun.JobRunState == 'RUNNING', then the run ends 'SUCCEEDED' and the queue receives the body 'completed:<run tag>' the script sends.
Actual (emulator): AssertionError: never observed the job in RUNNING state (check body line 65) — the polling loop ran to a terminal state with RUNNING never reported.

## Steps to Reproduce
1. Create an SQS queue and an IAM role trusting glue.amazonaws.com with sqs:SendMessage on it.
2. Upload a pythonshell script that reads --QUEUE_URL/--RUN_TAG, sleeps 25 seconds, then sends 'completed:<RUN_TAG>' to the queue; glue.create_job with Command Name='pythonshell'.
3. glue.start_job_run(JobName=..., Arguments={'--QUEUE_URL':..., '--RUN_TAG':...}).
4. Poll `aws glue get-job-run --job-name ... --run-id ...` every second until JobRunState is terminal, recording every state seen.
5. Observe that 'RUNNING' never appears among the observed states (and that no completion message reaches the queue).
