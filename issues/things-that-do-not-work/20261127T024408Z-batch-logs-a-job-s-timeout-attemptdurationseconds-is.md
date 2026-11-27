Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: batch__logs.batch-attempt-timeout-kills-container-before-completion-marker
Issue-Key: batch__logs.batch-attempt-timeout-kills-container-before-completion-marker|run#24|AssertionError|assert not any("DONE" in m for m in victim_msgs), (
Issue-Commit: 34ec76825aeb60426f74f39ee1ee1124c6afb7e4

# batch__logs: a job's timeout.attemptDurationSeconds is never enforced — the container runs to completion and the job ends SUCCEEDED instead of being killed and FAILED

> _Note: this issue was identified against the code base at commit `34ec76825aeb60426f74f39ee1ee1124c6afb7e4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SubmitJob

## Description
Cause 1 of 1: a job submitted with `timeout.attemptDurationSeconds` is allowed to run past that timeout. The container is no longer stopped at the deadline, so work AWS would have killed completes, and the job's terminal status and reason are those of a normal exit.

AWS: Smithy — `SubmitJob`/`JobDetail` carry `timeout` of shape `JobTimeout`, whose only member `attemptDurationSeconds` is documented as "The job timeout time (in seconds) that's measured from the job attempt's `startedAt` timestamp. After this time passes, Batch terminates your jobs if they aren't finished. The minimum value for the timeout is 60 seconds." So at `startedAt + attemptDurationSeconds` Batch terminates the attempt: the container is signalled mid-run, the job settles `FAILED` (not `SUCCEEDED`), and `JobDetail.statusReason` — modelled as "A short, human-readable string to provide more details for the current status of the job" — attributes the failure to the timeout rather than to the container's own exit (the recorded reason text names the timeout; AWS's sentence for this class is the attempt-duration-exceeded reason, with the job's own timeout value as its variable part). `stoppedAt - startedAt` is therefore bounded by the timeout, not by the command's natural duration. Anything the command would only have produced after the deadline — here a marker written by `sh -c "echo START; sleep 300 && echo DONE"`, where `&&` means DONE is written only if the sleep RETURNED — never reaches the `awslogs` stream.

Emulator: the timeout has no effect. With `attemptDurationSeconds=60` over a 300 s sleep, the job's own log stream holds BOTH markers — `['START', 'DONE']` — so the sleep returned and the container ran to completion; the attempt was never terminated at the deadline. The control job in the same check (5 s sleep, 3600 s timeout) behaves identically, which is exactly the point: the only thing that differs between the two submissions is the timeout, and it changes nothing. The check fails at its first timeout-specific assertion, so the downstream contract (status `FAILED`, a timeout-attributed `statusReason`/`container.reason`, and `stoppedAt - startedAt < 300 s`) is also unmet.

Why: a client that relies on Batch to bound an attempt gets none. A runaway or hung job is never reaped, so a queue's capacity is held indefinitely instead of being released by the timeout; a job that AWS reports as FAILED-on-timeout is reported SUCCEEDED here, so retry/alarm logic keyed on that verdict never fires and partial work past the deadline is treated as a complete, successful run.

Fix: the Batch job-attempt lifecycle in the job scheduler/runner — the component that starts an attempt and records `startedAt` must arm the attempt deadline from `timeout.attemptDurationSeconds`, stop the container when it elapses, and settle the job FAILED with the timeout-attributed `statusReason`; `DescribeJobs` then reports that status, reason and `stoppedAt` from state.

Traces: batch__logs.batch-attempt-timeout-kills-container-before-completion-marker. Related open issues that do NOT own this check: 20261119T015949Z-batch-2-net-new-causes-timeout-attemptdurationseconds-is.md and 20261125T025526Z-batch-logs-3-net-new-causes-a-job.md describe attemptDurationSeconds non-enforcement when the container had zero duration / never ran; here the container demonstrably RUNS (START and DONE both land in the log stream), so the remaining defect is the missing attempt deadline itself.

## Evidence
batch__logs.batch-attempt-timeout-kills-container-before-completion-marker

AssertionError: DONE is written only when the 300 s sleep returns, so a job killed at 60 s must never write it: ['START', 'DONE']

Setup replayed: one Fargate job definition, command `sh -c "echo START; sleep 300 && echo DONE"`, awslogs to a fresh log group.
  - control submission: command overridden to `sleep 5`, timeout={'attemptDurationSeconds': 3600} — SUCCEEDED and both markers delivered (proves the run and log-read path work).
  - victim submission: same job definition (300 s sleep), timeout={'attemptDurationSeconds': 60}.

Expected (AWS): victim job status FAILED; statusReason/container.reason naming the timeout; its own log stream contains START and never DONE; stoppedAt - startedAt < 300000 ms.
Actual (emulator): the victim's stream contains ['START', 'DONE'], i.e. the 300 s sleep returned — the attempt was never terminated at 60 s.

## Steps to Reproduce
1. batch.RegisterJobDefinition for a Fargate container with command ["sh","-c","echo START; sleep 300 && echo DONE"] and an awslogs logConfiguration pointing at a fresh CloudWatch Logs group.
2. batch.SubmitJob on an enabled Fargate queue with timeout={"attemptDurationSeconds": 60}.
3. Poll batch.DescribeJobs until the job is terminal.
4. Read the job's container.logStreamName with logs.GetLogEvents/FilterLogEvents.
   AWS: status FAILED, statusReason attributes the failure to the attempt timeout, the stream holds START only, and stoppedAt-startedAt is about 60 s.
   Emulator: the stream holds START and DONE (the 300 s sleep returned), so the timeout was never applied.
