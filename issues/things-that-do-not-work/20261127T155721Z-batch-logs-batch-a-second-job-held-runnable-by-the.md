Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: batch__logs.batch-maxvcpus-cap-holds-second-job-runnable-then-runs-it
Issue-Key: batch__logs.batch-maxvcpus-cap-holds-second-job-runnable-then-runs-it|_poll_until#6|Timeout|time.sleep(interval_s)
Issue-Commit: 7300602cd0fc37394a6328fdd869591f4a36c031

# batch: a second job held RUNNABLE by the compute environment's maxvCpus cap never starts once capacity frees up — the check never observes it run

> _Note: this issue was identified against the code base at commit `7300602cd0fc37394a6328fdd869591f4a36c031`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SubmitJob

## Description
Cause 1 of 1: the maxvCpus admission cap holds a second job RUNNABLE but the job never progresses to STARTING/RUNNING after the first job finishes and releases its vCPUs.

AWS: a Fargate compute environment with `maxvCpus` set admits jobs up to that cap. A job that would exceed it stays in `RUNNABLE` (DescribeJobs `status=RUNNABLE`) and is dispatched as soon as the running job reaches a terminal state and its vCPUs are released; it then reaches `RUNNING` and finally `SUCCEEDED`. Smithy: `SubmitJob`/`DescribeJobs` model the `JobStatus` enum SUBMITTED|PENDING|RUNNABLE|STARTING|RUNNING|SUCCEEDED|FAILED; the admission rule itself is documented behavior, not modelled.

Emulator: the check makes no further observable progress and the run ends with `Check timed out after 60.0s` — the held job is never dispatched after the first one completes, so no state transition out of RUNNABLE is ever observed. Hypothesis: the scheduler releases capacity on job completion but does not re-evaluate the queue, so a job parked by the cap is never reconsidered. Note the open issue 20261119T120645Z-batch-logs-queued-jobs-are-never-dispatched-a.md describes queued jobs never being dispatched at all; it does not carry this check's replay key, and the cap-release path is a distinct admission step.

Why: any client that sizes a compute environment and relies on Batch to serialise work past the cap sees its queue stall permanently. Throughput tests, array/queue drain logic and anything waiting on SUCCEEDED for the held job hang instead of completing.

Fix: the Batch job scheduler/dispatch loop — re-evaluate the RUNNABLE queue against free vCPUs whenever a job reaches a terminal state.

Traces: batch__logs.batch-maxvcpus-cap-holds-second-job-runnable-then-runs-it.

## Evidence
Expected: the second job stays RUNNABLE while the first runs, then transitions to RUNNING and SUCCEEDED once the first job's vCPUs are released.
Actual: Error: Check timed out after 60.0s — no transition out of RUNNABLE is ever observed.

## Steps to Reproduce
1. Create a Fargate compute environment with maxvCpus sized for exactly one job, plus a job queue and a job definition.
2. SubmitJob twice.
3. Poll DescribeJobs: the first job runs, the second stays RUNNABLE, and after the first reaches SUCCEEDED the second never leaves RUNNABLE.
