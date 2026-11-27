Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: batch__logs.batch-maxvcpus-cap-holds-second-job-runnable-then-runs-it
Issue-Key: batch__logs.batch-maxvcpus-cap-holds-second-job-runnable-then-runs-it|run#6|AssertionError|assert timeline and timeline[-1][1] == "RUNNING", (
Issue-Commit: f26705532556bda9b53da6f6657203498f448a8c

# batch__logs: a container job's RUNNING window collapses — the container command's own duration (sleep from containerOverrides.environment) is ignored and the job SUCCEEDS within ~20s

> _Note: this issue was identified against the code base at commit `f26705532556bda9b53da6f6657203498f448a8c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SubmitJob

## Description
Cause 1 of 1: a submitted Fargate container job reaches RUNNING but leaves RUNNING almost immediately, regardless of how long its container command actually takes to run. Failing check: batch__logs.batch-maxvcpus-cap-holds-second-job-runnable-then-runs-it.

AWS: a Batch job stays in RUNNING for as long as its container process runs, and only then transitions to a terminal status. Smithy (com.amazonaws.batch): JobStatus = SUBMITTED|PENDING|RUNNABLE|STARTING|RUNNING|SUCCEEDED|FAILED, and DescribeJobs reports the job's current status plus its attempt's startedAt/stoppedAt; SubmitJob accepts containerOverrides.environment, whose values are visible to the container process. The job definition under test runs ["sh","-c","echo BEGIN=$MARKER; sleep $HOLD; echo DONE=$MARKER"] with HOLD bound per job through containerOverrides.environment, so a job handed HOLD=150 must be observable in RUNNING for ~150 seconds (minus startup) before it reaches SUCCEEDED, and the duration is a property of the command and the environment it was handed, not a fixed emulator interval. Recorded behavior only for the exact timing; the rule is "terminal status follows the container exiting".

Emulator: the first job reaches RUNNING, then is already SUCCEEDED on the very next poll window. Replay timeline (t_s, first, second) from DescribeJobs: [(0,'RUNNING','SUBMITTED'), (5,'RUNNING','RUNNABLE'), (10,'RUNNING','RUNNABLE'), (15,'RUNNING','RUNNABLE'), (20,'SUCCEEDED','RUNNING')] — the job handed HOLD=150 left RUNNING after at most 20s, so the `sleep 150` the container was told to perform was never performed (or the per-job containerOverrides.environment never reached the container and HOLD was unset/empty). The job is nonetheless reported SUCCEEDED, i.e. the emulator claims a successful container execution it did not run for the commanded duration.

Why: a client cannot observe any steady state of a running job. Anything that depends on a job occupying capacity for a known interval breaks: compute-environment maxvCpus serialisation cannot be observed (the capacity window closes before a second job can be seen held), TerminateJob/CancelJob of a RUNNING job has no window to act in, attempt duration / timeout.attemptDurationSeconds has nothing to measure against, and log-stream ordering assertions ("the second container's first line is no earlier than the first container's last") are unverifiable. Worse, SUCCEEDED is a false claim: workloads whose real work is the elapsed container run are reported complete without having run.

Fix: the Batch job-execution/runtime component that drives a placed job's attempt — the RUNNING→terminal transition must be driven by the container process actually exiting (with the per-job containerOverrides.environment bound into that process), not by a fixed short timer after placement.

Related open issues that do NOT cover this one: 20261127T155721Z-batch-logs-batch-a-second-job-held-runnable-by-the.md carries this same check id but a different failure (the held job never leaving RUNNABLE after capacity freed); in this replay the second job DOES reach RUNNING once the first terminates, and the failure has moved to the first job's collapsed RUNNING window. 20261126T012211Z-batch-5-net-new-causes-a-running-job.md describes a collapsing RUNNING duration under service `batch`; it does not carry this check's replay key and this check is reported under `batch__logs`.

Traces: batch__logs.batch-maxvcpus-cap-holds-second-job-runnable-then-runs-it.

## Evidence
Expected: with a job definition whose command is `sh -c 'echo BEGIN=$MARKER; sleep $HOLD; echo DONE=$MARKER'` and containerOverrides.environment HOLD=150, DescribeJobs reports status=RUNNING for the whole ~150s the container sleeps, and only afterwards SUCCEEDED.
Actual: AssertionError (check line 83): "the first container must stay RUNNING for the whole observation window, otherwise the second job's status says nothing about capacity; it left RUNNING after 20s. Timeline (t_s, first, second): [(0, 'RUNNING', 'SUBMITTED'), (5, 'RUNNING', 'RUNNABLE'), (10, 'RUNNING', 'RUNNABLE'), (15, 'RUNNING', 'RUNNABLE'), (20, 'SUCCEEDED', 'RUNNING')]" — the job is SUCCEEDED after at most 20s instead of running for the commanded duration. (Poll instants and the 20s figure are this run's sampling values; the requirement is that the RUNNING window match the container's own duration.)

## Steps to Reproduce
1. Create a Fargate compute environment (maxvCpus=1), a job queue over it, and a log group.
2. RegisterJobDefinition with command ["sh","-c","echo BEGIN=$MARKER; sleep $HOLD; echo DONE=$MARKER"], vcpu=1, memory=2048, awslogs to that group.
3. SubmitJob with containerOverrides.environment MARKER=FIRST, HOLD=150; poll DescribeJobs until status=RUNNING.
4. Keep polling DescribeJobs every 5s: the job reports SUCCEEDED within ~20s of RUNNING instead of staying RUNNING for ~150s.
