Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: batch.job-queue-snapshot-capacity-usage-reflects-real-running-job batch.job-queue-priority-scheduling-order-enforced
Issue-Shape-Id: com.amazonaws.batch#AWSBatchV20160810
Issue-Key: batch.job-queue-snapshot-capacity-usage-reflects-real-running-job|run#33|AssertionError|assert vcpu_entries, f"expected a VCPU capacity usage entry, got {total_usage!r}"
Issue-Key: batch.job-queue-priority-scheduling-order-enforced|_poll_until#21|Timeout|return last
Issue-Commit: 7b83df792075b2a128316f9b5135ddba9a81dab1

# batch: 2 net-new causes — GetJobQueueSnapshot reports no capacity usage for a dispatched container job, and cross-queue priority ordering never reaches a verdict

> _Note: this issue was identified against the code base at commit `7b83df792075b2a128316f9b5135ddba9a81dab1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetJobQueueSnapshot

## Description
Two net-new root causes in the `batch` service's queue/scheduler reporting surface. Both were reached on a MANAGED/FARGATE compute environment the emulator DID place jobs onto (one job observed RUNNING, two RUNNABLE), so neither is the "jobs are never placed" family already filed under 20261120T110741Z-batch-2-net-new-causes-a-job-on.md / 20261119T015949Z-batch-2-net-new-causes-timeout-attemptdurationseconds-is.md — those issues do not carry either replay key below.

Cause 1 of 2: GetJobQueueSnapshot's queueUtilization.totalCapacityUsage is an empty list while a job is dispatched on the queue.
AWS: Smithy (services/service-com-amazonaws-batch-awsbatchv20160810/smithy-model/com-amazonaws-batch-awsbatchv20160810.json) models GetJobQueueSnapshot output member `queueUtilization` → QueueSnapshotUtilizationDetail with `totalCapacityUsage` : list<QueueSnapshotCapacityUsage{capacityUnit: String, quantity: Double}>. The operation's modelled doc states it provides "capacity utilization for already dispatched jobs" and that "Capacity utilization for the job queue is provided". So for a queue with one dispatched (RUNNING) container job whose resourceRequirements declare VCPU=1, AWS reports a totalCapacityUsage entry with capacityUnit "VCPU" and quantity equal to the vCPUs held by the queue's dispatched jobs (here 1); the quantity is a value derived from the request's job definitions and the emulator's state, not a fixed number.
Emulator: with DescribeJobs reporting exactly 1 RUNNING and 2 RUNNABLE 1-vCPU jobs on the queue (frontOfQueue already matched the RUNNABLE set, so the preceding assertions passed), GetJobQueueSnapshot answered 200 with queueUtilization.totalCapacityUsage == [] — no VCPU entry at all, and therefore no MEMORY entry either. The check failed on `assert vcpu_entries, f"expected a VCPU capacity usage entry, got {total_usage!r}"` → `AssertionError: expected a VCPU capacity usage entry, got []`.
Why: a client that sizes a queue or decides whether to submit more work from queueUtilization.totalCapacityUsage sees a permanently idle queue — zero vCPU held no matter how many jobs are dispatched — so capacity-aware schedulers, dashboards and autoscaling logic built on the snapshot read 0 utilization against a full compute environment.
Fix: the capacity accounting the GetJobQueueSnapshot handler reads — `state::queue_capacity_usage` as called from services/service-com-amazonaws-batch-awsbatchv20160810/src/ops/get_job_queue_snapshot.rs (the `total_capacity_usage` binding), which evidently accumulates nothing for dispatched CONTAINER jobs (the handler's service-job dispatch path is advanced separately). The handler's serialization is fine; the state that feeds it is empty.
Traces: batch.job-queue-snapshot-capacity-usage-reflects-real-running-job.

Cause 2 of 2: a two-queue, one-slot priority scenario never reaches a verdict — cross-queue priority is unobservable inside the replay budget.
AWS: recorded behavior only (the model carries no scheduling-timing contract; CreateJobQueue's `priority` is documented as deciding dispatch order among queues sharing a compute environment). Against AWS the scenario settles: a filler job on the low-priority queue occupies the single 1-vCPU Fargate slot and reaches RUNNING then SUCCEEDED, the two probe jobs sit RUNNABLE, and when the slot frees the job on the priority-100 queue leaves RUNNABLE before the job on the priority-1 queue; both then reach SUCCEEDED.
Emulator: the check produced no observation at all — `Check timed out after 60.0s`. The emulator never carried the filler job through RUNNING→SUCCEEDED (freeing the slot) fast enough for the scenario to reach the comparison, so whether queue priority gates placement at all cannot be observed.
Why: a client that relies on queue priority to pre-empt a backlog gets no guarantee from this emulator: the higher-priority queue's job may never advance first, and the ordering is simply unverifiable. Job lifecycle transitions that only complete on wall-clock timescales also make every capacity-cycling scenario reach the harness budget instead of a verdict.
Fix: the container-job dispatch/state machine in the batch crate's state (the placement step that frees a slot when a dispatched job terminates and then selects the next RUNNABLE job across the queues attached to one compute environment, ordered by jobQueue.priority), not the GetJobQueueSnapshot handler.
Traces: batch.job-queue-priority-scheduling-order-enforced.

## Evidence
Cause 1 — check batch.job-queue-snapshot-capacity-usage-reflects-real-running-job:
  expected: GetJobQueueSnapshot → queueUtilization.totalCapacityUsage contains an entry {capacityUnit: "VCPU", quantity: 1} while one 1-vCPU job is RUNNING on the queue (two siblings RUNNABLE).
  actual:   200 with queueUtilization.totalCapacityUsage == []
  assertion: AssertionError: expected a VCPU capacity usage entry, got []
  (DescribeJobs at the same instant reported exactly 1 RUNNING / 2 RUNNABLE, and frontOfQueue held exactly the two RUNNABLE jobArns, so the divergence is isolated to queueUtilization.)

Cause 2 — check batch.job-queue-priority-scheduling-order-enforced:
  expected: filler job RUNNING→SUCCEEDED on the priority-1 queue, probe jobs RUNNABLE, then the priority-100 queue's job leaves RUNNABLE first and both jobs end SUCCEEDED.
  actual:   Check timed out after 60.0s — no state transition observation reached; the scenario never got past waiting for the dispatched filler job to settle.

## Steps to Reproduce
1. CreateComputeEnvironment: type=MANAGED, state=ENABLED, computeResources{type=FARGATE, maxvCpus=1, subnets=[<subnet>], securityGroupIds=[<sg>]}; wait for status VALID.
2. Cause 1: CreateJobQueue (priority 1) on that CE; RegisterJobDefinition type=container, platformCapabilities=[FARGATE], containerProperties.resourceRequirements=[{VCPU,"1"},{MEMORY,"2048"}], command ["sleep","90"]. SubmitJob three times. Poll DescribeJobs until exactly 1 RUNNING and 2 RUNNABLE. Call GetJobQueueSnapshot(jobQueue=<queue>) and read queueUtilization.totalCapacityUsage — it is [] instead of carrying a VCPU entry of quantity 1.
3. Cause 2: on the same 1-vCPU CE create two queues, priority 1 and priority 100. SubmitJob a "sleep 30" filler to the low-priority queue and wait for RUNNING; then SubmitJob a probe job to the low-priority queue and another to the high-priority queue, confirm both RUNNABLE, wait for the filler to reach SUCCEEDED, and watch which probe leaves RUNNABLE first. The scenario never reaches that observation before the replay budget expires.
