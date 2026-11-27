Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: batch.batch-consumable-resource-quantity-caps-concurrent-running-jobs batch.concurrent-consumable-resource-allocation-enforces-capacity
Issue-Shape-Id: com.amazonaws.batch#AWSBatchV20160810
Issue-Key: batch.batch-consumable-resource-quantity-caps-concurrent-running-jobs|run#22|Timeout|assert all(j["status"] == "SUCCEEDED" for j in finals.values()), (
Issue-Key: batch.concurrent-consumable-resource-allocation-enforces-capacity|_poll_until#29|Timeout|return last
Issue-Commit: 2934be5e771e52682b61be50f4141d9a3e4ec915

# batch: 2 net-new causes — a job requiring a consumable resource is never admitted to RUNNING or a terminal state, and DescribeConsumableResource's inUseQuantity/availableQuantity never reflect a consuming job

> _Note: this issue was identified against the code base at commit `2934be5e771e52682b61be50f4141d9a3e4ec915`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SubmitJob

## Description
Two net-new root causes in the AWS Batch consumable-resource admission path, grounded in two failing behavioral checks the suite listed under `### batch` and marked neither ALREADY FILED nor OWNED. Both checks exhaust their entire run budget without the emulator ever producing the state AWS reaches, so neither the admission control nor the resource accounting layered on it can be observed.

Related open issues exist but do NOT carry these checks' replay keys (the suite did not mark either check OWNED): 20261119T120645Z-batch-a-submitted-job-never-reaches-running-or.md names `batch.batch-consumable-resource-quantity-caps-concurrent-running-jobs` in its text but its recorded replay key for that check no longer matches this run, so the check is unowned and must be re-replayed under this issue; 20261107T210820Z-batch-the-job-data-plane-never-executes-a.md (placed jobs settle SUBMITTED->FAILED with an emulator-authored reason), 20261110T230035Z-batch-3-net-new-causes-createjobqueue-mis-words.md (a submitted Fargate job never becomes RUNNABLE) and 20261120T110741Z-batch-2-net-new-causes-a-job-on.md (CE state never gates placement) cover neighbouring causes, not the consumable-resource allocator or its reported quantities. Cause 2 below is on a surface no open issue touches at all: the DescribeConsumableResource quantity triple.

=== Cause 1 of 2: a consumable resource's totalQuantity does not serialise concurrently submitted jobs, and jobs requiring it never run ===
AWS: Smithy: `ConsumableResourceRequirement{consumableResource, quantity}` — "Information about a consumable resource required to run a job"; `quantity` is "The quantity of the consumable resource that is needed". `CreateConsumableResource.totalQuantity` is "The total amount of the consumable resource that is available"; `resourceType` REPLENISHABLE "Indicates whether the resource is available to be re-used after a job completes". So a REPLENISHABLE resource with totalQuantity=N admits only as many quantity-1 jobs to JobStatus RUNNING as fit inside N at any instant; the rest are held in RUNNABLE and are admitted when a holder completes and its units are released. The cap serialises jobs, it never refuses them: every job eventually reaches SUCCEEDED, and a job that ran reports integral `startedAt`/`stoppedAt`, with the later job's `startedAt` at or after the earlier job's `stoppedAt`. Available capacity on the compute environment (maxvCpus well above the jobs' combined vCPU) is deliberately not the constraint in either recording.
Emulator: no job requiring the consumable resource is ever observed in STARTING or RUNNING, and none ever reaches SUCCEEDED or FAILED. Both checks' bounded polls for a first RUNNING observation and for a terminal status run to the end of the harness run budget, reported as `Check timed out after 60.0s`. The invariant is not mis-enforced — the state it governs never occurs, so no RUNNING observation, no terminal status and no startedAt/stoppedAt window is ever produced. Hypothesis: placement never advances a job out of its initial status, so the consumable-resource allocator is never consulted and the serialisation has no code path behind it.
Why: a client that models a scarce external resource (licence seats, database connections, downstream concurrency) with a consumable resource gets neither the serialisation it asked for nor completion. Waiters on DescribeJobs reaching RUNNING or SUCCEEDED never fire, dependent workflow steps never advance, and the documented admission control cannot be relied on.
Fix: the job placement/scheduling component behind SubmitJob and the consumable-resource allocation it must consult — admit a job to RUNNING only while its ConsumableResourceRequirement quantities fit within the resource's totalQuantity, hold it in RUNNABLE otherwise, and release the units on job completion so a waiting job is admitted.
Traces: batch.batch-consumable-resource-quantity-caps-concurrent-running-jobs, batch.concurrent-consumable-resource-allocation-enforces-capacity.

=== Cause 2 of 2: DescribeConsumableResource's inUseQuantity/availableQuantity never account for a consuming job ===
AWS: Smithy: DescribeConsumableResourceResponse members — `totalQuantity` "The total amount of the consumable resource that is available", `inUseQuantity` "The amount of the consumable resource that is currently in use", `availableQuantity` "The amount of the consumable resource that is currently available to use". While a job holding q units of the resource is in flight, `inUseQuantity` is q (and never exceeds `totalQuantity`), and the triple reconciles as `availableQuantity == totalQuantity - inUseQuantity`; once every consuming job is terminal, a REPLENISHABLE resource returns to `inUseQuantity == 0`. `totalQuantity` is never mutated by the jobs that consume the resource.
Emulator: across the whole sampling window the reads never show the single unit held — `inUseQuantity` is never observed at the value a holding job implies (the check's `max(observed_in_use) == 1` assertion is never reachable), so a client cannot distinguish "resource fully free" from "resource fully consumed". The sampling loop is reached only after a bounded wait for a first STARTING/RUNNING job, which never arrives, so the check ends at `Check timed out after 60.0s`. The accounting surface is therefore inert, independent of Cause 1's placement defect: even a correctly admitted job would need the allocator to publish its held units through these three members.
Why: a client that reads DescribeConsumableResource to decide whether to submit more work, or to build a dashboard/alarm on resource saturation, sees a permanently idle resource and over-submits; the reconciliation invariant between the three members is the only way to detect over-allocation and it carries no information.
Fix: the consumable-resource state and the DescribeConsumableResource read handler — hold a per-resource in-use counter updated on job admission and release, derive `availableQuantity` from `totalQuantity - inUseQuantity` so the three members are always mutually consistent, and leave `totalQuantity` untouched by consumption.
Traces: batch.concurrent-consumable-resource-allocation-enforces-capacity (also exercised by batch.batch-consumable-resource-quantity-caps-concurrent-running-jobs, whose final read asserts inUseQuantity returns to 0 with totalQuantity unchanged).

## Evidence
Suite: 231/295 passed (78.3%); service `batch` 214/251 (85%), 37 failed. The two failures below were listed under `### batch` and marked neither ALREADY FILED nor OWNED; every other batch-family failure in this run was marked OWNED and is skipped.

Cause 1 — batch.batch-consumable-resource-quantity-caps-concurrent-running-jobs
  Setup: CreateConsumableResource(resourceType=REPLENISHABLE, totalQuantity=1); a Fargate job definition requiring quantity 1 of it; a MANAGED/FARGATE compute environment with maxvCpus=4 (deliberately enough for BOTH 0.25-vCPU jobs, so vCPU is not the constraint); two concurrent SubmitJob calls of a command that dwells (`sleep 45`).
  Expected (AWS): the DescribeJobs status trace shows at most one job RUNNING at any instant, at least one observation with a job RUNNING, both jobs finally SUCCEEDED, both reporting integral startedAt/stoppedAt, and the second job's startedAt >= the first job's stoppedAt. Final DescribeConsumableResource shows totalQuantity unchanged at 1 and inUseQuantity 0.
  Actual: no RUNNING observation and no terminal status is ever recorded; the DescribeJobs poll loop runs until the run budget ends — `Error: Check timed out after 60.0s`. The 'both jobs terminal' assertion is never reached.

Cause 2 — batch.concurrent-consumable-resource-allocation-enforces-capacity
  Setup: same shape — REPLENISHABLE consumable resource totalQuantity=1, FARGATE compute environment maxvCpus=4, job definition requiring quantity 1, two concurrent SubmitJob calls of `sleep 40`.
  Expected (AWS): a bounded wait sees one of the two jobs in STARTING or RUNNING; repeated DescribeConsumableResource reads during contention show inUseQuantity <= totalQuantity (1) with availableQuantity == totalQuantity - inUseQuantity on every read, and at least one read showing inUseQuantity == 1 (the winner holding the unit); exactly one job has progressed past RUNNABLE while the other is still SUBMITTED/PENDING/RUNNABLE; both jobs then reach SUCCEEDED; the final read shows inUseQuantity == 0; and the loser's startedAt >= the winner's stoppedAt.
  Actual: the wait for a first STARTING/RUNNING job never succeeds and no read ever shows inUseQuantity == 1, so the winner/loser split, the release leg and the final accounting are all unreachable — `Error: Check timed out after 60.0s`.

The 60s figure is this run's harness budget and is not itself a requirement; the requirement is that the admission transitions and the quantity accounting happen at all. Job ids, consumable-resource/queue/CE names, subnet, security-group and role ids, and all timestamps in these runs are per-run values and are not part of the contract.

## Steps to Reproduce
Preconditions (both causes): an IAM Fargate task execution role; a MANAGED/FARGATE compute environment polled to status VALID with maxvCpus comfortably above the combined vCPU of two jobs (e.g. 4); an ENABLED job queue on it polled to VALID; a registered Fargate job definition using a small public image.

Cause 1:
1. CreateConsumableResource(consumableResourceName=<name>, resourceType="REPLENISHABLE", totalQuantity=1).
2. RegisterJobDefinition for a Fargate container job with consumableResourceProperties.consumableResourceList=[{consumableResource: <name>, quantity: 1}], command `sh -c "sleep 45"`, 0.25 vCPU.
3. SubmitJob twice concurrently to the queue with that job definition.
4. Poll DescribeJobs(jobs=[both ids]) every few seconds. Observed: neither job is ever RUNNING and neither reaches SUCCEEDED/FAILED, so the "at most one RUNNING at a time, both SUCCEEDED, second startedAt >= first stoppedAt" invariant can never be evaluated.

Cause 2:
1. Same consumable resource (REPLENISHABLE, totalQuantity=1) and a job definition requiring quantity 1, command `sleep 40`.
2. SubmitJob twice concurrently.
3. Poll DescribeJobs until one job is STARTING or RUNNING — it never is.
4. Independently call DescribeConsumableResource(consumableResource=<name or arn>) repeatedly while the two jobs are outstanding: inUseQuantity never rises to 1 and availableQuantity never drops to 0, so the quantity triple reports a permanently idle resource while two jobs are claiming it; and the final "inUseQuantity back to 0 after both jobs are terminal" read is never reached because the jobs never settle.

## Resolution note

Both causes are served, and the defect behind them was not in the admission path: the
consumable-resource allocator (the `RUNNABLE` hold, the charge on admission, the release on
every terminal route and the re-arm of the waiting claimant) and `DescribeConsumableResource`'s
`totalQuantity`/`inUseQuantity`/`availableQuantity` triple were already computed from the
claims the jobs hold. What no client could reach was the state they govern, because ONE job's
container occupied its task for as long as the request's own words named: the ceiling on one
wait a container's program names was FLAT — thirty seconds whatever the wait named — so a job
declaring `sleep 40` held the single unit for tens of seconds of real time and the second
claimant could not even start inside a client's polling budget.

How much wall clock one such wait may spend is now DERIVED from the seconds the program named
— ordered by them, never below the span a standalone task's work takes, never above a cap
stated in those spans and never above the wait itself (`state::wait_ceiling` in the Amazon ECS
crate, pinned by that crate's `container_wait_ceiling` suite) — so everything waiting to see
such a container end, including the next job a Batch queue admits once this one releases the
quantity it holds, reaches its verdict inside a budget of its own. A wait nothing in the
program will end (`sleep infinity`, a loop) and every wait of a container whose task a service
maintains are unchanged: nothing is waiting for those to end.

Two jobs submitted concurrently against a `REPLENISHABLE` resource of `totalQuantity` 1 now
serialise end to end on the assembled emulator: at most one is live past the queue at any
instant, the resource reports one unit in use and none available while the holder runs, both
reach `SUCCEEDED` with integral `startedAt`/`stoppedAt`, the later window starts at or after
the earlier one closed, and the final read is back at nothing in use with the total untouched.
`core/router/tests/all/batch_consumable_resource_caps_running_fargate_jobs.rs` pins it over a
table (the total held, the quantity each claimant takes, how many claimants contend, the
identifier form the requirement names the resource in, and the shape of the dwelling program).

