Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: batch__logs.batch-sequential-array-children-run-one-at-a-time-in-index-order
Issue-Key: batch__logs.batch-sequential-array-children-run-one-at-a-time-in-index-order|_paginate#6|Timeout|return out
Issue-Commit: 38f62366cd2817d69083419007bb0aafef336c94

# batch__logs: a SEQUENTIAL array job's children never execute — the array never settles and index-ordered serialisation, per-child exit codes and per-child awslogs streams are all unobservable

> _Note: this issue was identified against the code base at commit `38f62366cd2817d69083419007bb0aafef336c94`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SubmitJob

## Description
One net-new root cause, grounded in the single failing behavioral check the suite lists under `### batch__logs` that it marks neither ALREADY FILED nor OWNED.

Related open issues that do NOT carry this check's replay key for this replay (so none of them covers it): 20261125T025526Z-batch-logs-3-net-new-causes-a-job.md — its Cause 3 describes this same scenario, but its recorded key for this check is `batch__logs.batch-sequential-array-children-run-one-at-a-time-in-index-order|_await_group_settled#6|Timeout|return view`, while on this replay the check dies earlier (it never reaches `_await_group_settled`: the array parent itself never becomes terminal), so the suite did not treat it as owned; 20261119T120645Z-batch-logs-queued-jobs-are-never-dispatched-a.md (its sequential-array key is a `_poll_until` timeout); 20261124T031653Z-batch-logs-in-a-sequential-array-job-no.md and 20261124T201100Z-batch-logs-a-sequential-array-job-whose-child.md (the failing-child negative face of the sequential array, not the positive ordering face reported here); 20261127T172359Z-batch-logs-a-container-job-s-running-window.md (the single-job RUNNING window).

=== Cause 1 of 1: array-job children of a SEQUENTIAL submission are never dispatched or executed, so the array never reaches a terminal status ===

AWS: Smithy (com.amazonaws.batch): `SubmitJob` takes `arrayProperties` and `dependsOn`; `JobDependency.type` targets `ArrayJobDependency`, the enum [N_TO_N, SEQUENTIAL]; `ArrayPropertiesDetail.size` is "The size of the array job. This parameter is returned for parent array jobs.", `ArrayPropertiesDetail.statusSummary` is "A summary of the number of array job children in each available job status. This parameter is returned for parent array jobs.", and `ArrayPropertiesDetail.index` is "The job index within the array that's associated with this job. This parameter is returned for array job children."; `JobStatus` is the enum [SUBMITTED, PENDING, RUNNABLE, STARTING, RUNNING, SUCCEEDED, FAILED]. In AWS a `dependsOn` entry carrying only `{"type": "SEQUENTIAL"}` (no `jobId`) on an array submission makes the children of that very submission run strictly one after another in ascending index order, even on capacity that could run them all concurrently (here a MANAGED/FARGATE compute environment with `computeResources.maxvCpus` well above the sum of the children's `resourceRequirements` VCPU, so capacity is demonstrably not what serialises them). Each child `<arrayJobId>:<i>` is enumerable via `ListJobs(arrayJobId=...)` across the job statuses, is readable by `DescribeJobs`, reports `arrayProperties.index == i`, actually runs its container with `AWS_BATCH_JOB_ARRAY_INDEX` expanded to its own index, ends `SUCCEEDED` with `container.exitCode` 0, and owns a distinct awslogs stream (the one named in `container.logStreamName`) holding exactly the lines its own container printed, one event per stdout line. The parent ends `SUCCEEDED` with `arrayProperties.statusSummary` accounting for every child as SUCCEEDED, and child i's first log event timestamp is not earlier than child i-1's last — that ordering is the whole guarantee of a SEQUENTIAL array dependency. All responses on this path are 200; there is no refusal involved.

Emulator: the check reaches no verdict at all — the replay reports `Error: Check timed out after 60.0s` while still polling `DescribeJobs` for the array parent to reach a terminal `JobStatus`. Nothing the array was supposed to produce materialises: the parent never becomes SUCCEEDED and never carries a `statusSummary` accounting for its children, the children are never enumerated/terminal, no child reports `arrayProperties.index`, no child reports `container.exitCode` 0, no per-child awslogs stream is created, and no `BEGIN=<i>` / `END=<i>` event is delivered. Because no child ever runs, there are no timestamps to compare, so the ascending-index serialisation the check exists to grade cannot be observed at all. (The compute environment and queue do reach VALID and `SubmitJob` is accepted with 200, so the stall is specifically on array-child scheduling/execution and awslogs delivery, not on the control-plane calls.)

Why: a client that expresses an ordered pipeline over array indices — the documented use of a SEQUENTIAL array dependency — gets neither execution nor ordering. Stage i+1 can never be known to follow stage i; any consumer tailing a child's stream named by `container.logStreamName` blocks forever or silently processes nothing; and a poller waiting for the parent's terminal status or for `arrayProperties.statusSummary` to settle never returns, so a workflow built on array jobs hangs rather than failing visibly.

Fix: the array-job child scheduling and execution behind the `SubmitJob` array path, plus the awslogs delivery path for Batch containers — a `dependsOn` entry of type SEQUENTIAL with no `jobId` must gate each child index on the termination of the preceding index and release the children in ascending index order; each child must actually run its container with `AWS_BATCH_JOB_ARRAY_INDEX` set to its own index and settle into a terminal `JobStatus` with `container.exitCode` and `arrayProperties.index` populated; the parent must settle with `arrayProperties.statusSummary` reflecting its children; and each child's stdout must be delivered line-by-line to the log stream it reports.

Traces: batch__logs.batch-sequential-array-children-run-one-at-a-time-in-index-order.

## Evidence
Suite: 1066/1450 passed (73.5%); service `batch__logs` 5/10 (50%), 5 failed. Four of those five are marked OWNED by existing issues; the one below is marked neither ALREADY FILED nor OWNED and is the cause above.

[mismatch] batch__logs.batch-sequential-array-children-run-one-at-a-time-in-index-order
  Error: Check timed out after 60.0s

  Expected (AWS), from the check body:
    - the array parent reaches a terminal JobStatus of SUCCEEDED, and (arrayProperties.statusSummary).SUCCEEDED equals the array size;
    - ListJobs over the array enumerates exactly `size` children named `<arrayJobId>:<i>`;
    - each child: DescribeJobs returns it, status == 'SUCCEEDED', container.exitCode == 0, arrayProperties.index == i, and container names a non-empty awslogs log stream;
    - the children's reported streams are pairwise distinct, and the log group holds exactly those streams and nothing else;
    - each child i's stream holds exactly its own two events, in order, with messages 'BEGIN=<i>' then 'END=<i>' (so an emulator that echoed the command back, or left $AWS_BATCH_JOB_ARRAY_INDEX unexpanded, must not pass);
    - for every i >= 1: timestamp(child i 'BEGIN=<i>') >= timestamp(child i-1 'END=<i-1>') — the SEQUENTIAL ordering guarantee.
  Actual (emulator): no verdict — the check consumes its whole run budget polling DescribeJobs for the array parent to become terminal. The array never settles, no child is ever terminal or enumerated, no per-child awslogs stream exists, and no BEGIN=/END= event is ever delivered, so none of the above can be asserted.

Per-run/environment values that are NOT requirements: the 60 s figure is this run's harness budget; the array size, the per-child sleep, the vCPU/memory figures, the VPC/subnet CIDRs, the busybox image tag, the generated job ids, ARNs, log-group and stream names and all timestamps are the check's own inputs or per-run values. The requirements are: a SEQUENTIAL array's children are dispatched and executed one at a time in ascending index order, each child settles terminal with its own exitCode and arrayProperties.index, each child owns and fills its own awslogs stream with its container's stdout, and the parent settles with a statusSummary accounting for all children.

## Steps to Reproduce
Preconditions: a CloudWatch Logs log group; an IAM Fargate task execution role trusted by ecs-tasks.amazonaws.com and allowed ecr:GetAuthorizationToken, ecr:BatchCheckLayerAvailability, ecr:GetDownloadUrlForLayer, ecr:BatchGetImage, logs:CreateLogStream, logs:PutLogEvents; a VPC with two subnets.

1. CreateComputeEnvironment MANAGED/FARGATE with computeResources.maxvCpus=4 over those subnets; poll DescribeComputeEnvironments to VALID.
2. CreateJobQueue ENABLED on that compute environment; poll DescribeJobQueues to VALID.
3. RegisterJobDefinition type=container, platformCapabilities=[FARGATE], image public.ecr.aws/docker/library/busybox:1.36, resourceRequirements VCPU 0.25 / MEMORY 512, executionRoleArn = the role above, logConfiguration logDriver=awslogs pointed at the log group, command ["sh","-c","echo BEGIN=$AWS_BATCH_JOB_ARRAY_INDEX; sleep 15; echo END=$AWS_BATCH_JOB_ARRAY_INDEX"]. (4 vCPUs against 3 x 0.25-vCPU children means capacity is not what serialises the children.)
4. SubmitJob once with arrayProperties={"size":3} and dependsOn=[{"type":"SEQUENTIAL"}] (a type with no jobId: the documented sequential-array form, where the dependency is between the children of this submission).
5. Poll DescribeJobs on the returned arrayJobId until it reports a terminal status.
6. Observe: the parent never reaches SUCCEEDED and never carries arrayProperties.statusSummary for its children; ListJobs(arrayJobId=..., over every jobStatus) never enumerates `<arrayJobId>:0..2`; DescribeJobs on those child ids never reports SUCCEEDED, container.exitCode 0 or arrayProperties.index; no child log stream appears in the log group and GetLogEvents/FilterLogEvents never return a BEGIN=<i> or END=<i> event. In AWS the parent and all three children settle SUCCEEDED, each child owns a distinct stream holding exactly 'BEGIN=<i>' then 'END=<i>', and child i's BEGIN is not earlier than child i-1's END.
