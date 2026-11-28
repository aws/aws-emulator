Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: batch__logs.batch-container-memory-limit-kills-job-before-completion-marker
Issue-Key: batch__logs.batch-container-memory-limit-kills-job-before-completion-marker|_describe_job#13|Timeout|return js[0] if js else None
Issue-Commit: 7b83df792075b2a128316f9b5135ddba9a81dab1

# batch__logs: a container's declared MEMORY ceiling is never enforced — the over-allocation-killed vs inside-the-limit comparison never reaches a verdict

> _Note: this issue was identified against the code base at commit `7b83df792075b2a128316f9b5135ddba9a81dab1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SubmitJob

## Description
Cause 1 of 1: the Fargate container-memory ceiling declared on a Batch job definition has no observable effect, and the scenario that would prove it never reaches a verdict.
AWS: recorded behavior only for the runtime effect (the Smithy model only carries the declaration: RegisterJobDefinition containerProperties.resourceRequirements accepts a MEMORY requirement, and for FARGATE the vCPU/memory pair is the task's hard limit). Against AWS the two jobs differ only in how much memory the same command allocates: the job whose shell allocates far past the declared MEMORY (512 MiB here) is killed by the runtime BEFORE its `ALLOC_DONE` completion marker is written, so its log stream shows `ALLOC_START` and no `ALLOC_DONE` and the job ends FAILED; the sibling job allocating comfortably inside the limit writes both markers and ends SUCCEEDED. The limit is enforced by the task runtime, not echoed by the API.
Emulator: the check produced no observation at all — `Check timed out after 60.0s`. Neither job's container outcome nor its CloudWatch Logs stream was ever reached, so the emulator exhibits no memory enforcement and no settled terminal status for either job inside the replay budget.
Why: a client validating that its job definition's memory sizing actually bounds the container gets no signal: an over-allocating job is indistinguishable from a well-behaved one, and a workload that would be OOM-killed in AWS appears to run (or never settles). Capacity planning and failure-handling logic written against this emulator is unverifiable.
Why this is net-new: the open batch__logs issues (20261119T003525Z-batch-logs-a-placed-job-s-container-never.md, 20261119T120645Z-batch-logs-queued-jobs-are-never-dispatched-a.md, 20261120T235656Z-batch-logs-terminatejob-of-a-running-job-is.md) carry other replay keys and none of them owns this check; the suite did not mark it as owned or already filed.
Fix: the container execution path of the Batch data plane in the batch crate's job state machine — the step that runs a placed job's container must apply the job definition's MEMORY resource requirement as a hard limit, terminate the container when it is exceeded (FAILED, completion marker never written) and settle both jobs to a terminal status promptly enough to be observed, while the log-group writer emits whatever the container produced before the kill.
Traces: batch__logs.batch-container-memory-limit-kills-job-before-completion-marker.

## Evidence
Check batch__logs.batch-container-memory-limit-kills-job-before-completion-marker:
  expected: two jobs from the same job definition shape (memory=512 MiB, Fargate), command `sh -c 'echo ALLOC_START; <awk allocating N bytes> && echo ALLOC_DONE'`.
           - N ≈ 1 GiB target (peak ~2x, far past 512 MiB): job ends FAILED, its log stream contains ALLOC_START and NOT ALLOC_DONE.
           - N = 64 MiB target (peak ~128 MiB, inside 512 MiB): job ends SUCCEEDED, log stream contains both ALLOC_START and ALLOC_DONE.
  actual:   Check timed out after 60.0s — no terminal job status and no log events were ever observed for either job; the memory ceiling produced no observable difference.

## Steps to Reproduce
1. CreateLogGroup for the job's awslogs destination; create an IAM execution role for ecs-tasks.amazonaws.com.
2. CreateComputeEnvironment MANAGED/FARGATE (maxvCpus=4) and CreateJobQueue on it; wait for VALID.
3. RegisterJobDefinition type=container, platformCapabilities=[FARGATE], containerProperties.resourceRequirements MEMORY="512" (plus a VCPU value), logConfiguration awslogs → the log group, command ["sh","-c","echo ALLOC_START; awk 'BEGIN{s=\"x\"; while(length(s)<1073741824) s=s s; print \"LEN=\" length(s)}' && echo ALLOC_DONE"].
4. SubmitJob and poll DescribeJobs to a terminal status, then read the job's CloudWatch Logs stream: expect FAILED with ALLOC_START present and ALLOC_DONE absent.
5. Repeat with the allocation target lowered to 67108864 (64 MiB): expect SUCCEEDED with both markers. Neither leg reaches a terminal observation against the emulator.
