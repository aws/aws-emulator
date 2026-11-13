Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__ecs__servicediscovery.ecs-run-task-is-blocked-by-insufficient-memory-on-the-only-container-instance
Issue-Key: ec2__ecs__servicediscovery.ecs-run-task-is-blocked-by-insufficient-memory-on-the-only-container-instance|run#16|AssertionError|assert refused["tasks"] == [], (
Issue-Commit: 3bfb7cd772781c7f8fd33f0bb610479e140a70f3

# ec2__ecs__servicediscovery: RunTask ignores container-instance resource capacity — an oversized task is placed instead of refused with a RESOURCE:MEMORY failure

> _Note: this issue was identified against the code base at commit `3bfb7cd772781c7f8fd33f0bb610479e140a70f3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunTask

## Description
Cause 1 of 1: ECS RunTask (launchType=EC2) performs no resource-capacity check against the registered container instances of the cluster, so a task definition demanding more memory than any container instance has is PLACED instead of being refused.

AWS: RunTask is a placement request, not an allocation request. ECS filters the cluster's ACTIVE container instances by their remaining registered resources (Smithy: ECS Resource shape — "The name of the resource, such as CPU, MEMORY, PORTS, PORTS_UDP, or a user-defined resource", carried by ContainerInstance.registeredResources / remainingResources). When no instance can satisfy the request, RunTask does NOT raise: it answers HTTP 200 with an EMPTY `tasks` list and exactly one entry in `failures` (Smithy: RunTaskResponse = {tasks: Tasks, failures: Failures}; Failure = {arn, reason, detail}, documented as "A failed resource. For a list of common causes, see API failure reasons"). The `reason` is the structured, fixed token `RESOURCE:MEMORY` — fixed text, with the resource name (`MEMORY`, or `CPU`/`PORTS` for the corresponding shortage) being the variable part. The `arn` is the ARN of the container instance that could not satisfy the request, in the SHORT container-instance form (no `/<clusterName>/` segment, unlike the ARN DescribeContainerInstances returns). No task record, no task id and no reservation is created for a refused placement.

Emulator: for a definition asking 8192 MiB against the only registered container instance (which registered a MEMORY value strictly between 128 and 8192 MiB — the check asserts this before proceeding), RunTask answered 200 with `failures` empty and ONE task in `tasks`: taskArn arn:aws:ecs:<region>:<account>:task/<cluster>/<id>, containerInstanceArn arn:aws:ecs:<region>:<account>:container-instance/<cluster>/<id>, lastStatus PENDING, launchType EC2, and the container reporting `memory: '8192'` on a host that never had 8192 MiB. The oversized task is accepted and recorded, i.e. placement is unconditional: whatever the cluster's capacity, every RunTask succeeds. (The right-sized 128 MiB control in the same check placed correctly, so this is not an empty-cluster artifact.)

Why: a client that relies on ECS's capacity accounting gets the opposite answer. Code that inspects `failures` for `RESOURCE:MEMORY` to scale out capacity, pick a smaller size, fall back to Fargate, or surface "insufficient capacity" never sees a failure and instead believes an impossible task was scheduled; conversely, tests that assert an oversized task is rejected pass against AWS and fail against the emulator. Because the refused task is also recorded in state, subsequent ListTasks/DescribeTasks report phantom workload that over-commits the host, so any capacity arithmetic built on top of it is wrong from then on.

Fix: ECS RunTask's placement path (the handler for RunTask and the container-instance resource accounting in ECS state): before creating a task record, select candidate instances by remaining registered resources (MEMORY, CPU, host PORTS) and, when none qualifies, return 200 with `tasks: []` and one `Failure{arn: <short container-instance ARN>, reason: "RESOURCE:MEMORY"}` while creating no task and leaving existing tasks untouched; the successful path must decrement the chosen instance's remaining resources so the accounting is consistent.

Related but distinct open issues (neither covers this capacity check): 20261113T030250Z-ec2-ecs-servicediscovery-an-ec2-launch-type-task.md (EC2 launch-type task carried no containerInstanceArn — the replay now shows one) and 20261109T013300Z-ec2-ecs-servicediscovery-an-ec2-instance-launched-from.md (container-instance registration).

Traces: ec2__ecs__servicediscovery.ecs-run-task-is-blocked-by-insufficient-memory-on-the-only-container-instance.

## Evidence
Check: ec2__ecs__servicediscovery.ecs-run-task-is-blocked-by-insufficient-memory-on-the-only-container-instance [mismatch]

Setup (all asserted before the divergence): one ECS cluster, one t3.micro container instance registered ACTIVE with 128 < registeredResources.MEMORY < 8192 MiB; two bridge/EC2 task definitions, one asking memory=128 (control) and one asking memory=8192.

Control RunTask(taskDefinition=<128 MiB family>, count=1, launchType=EC2): passed — 1 task, 0 failures, placed on the expected containerInstanceArn.

Oversized RunTask(taskDefinition=<8192 MiB family>, count=1, launchType=EC2):
  Expected (AWS): HTTP 200, tasks == [], failures == [{"arn": "arn:aws:ecs:<region>:<account>:container-instance/<instance-uuid>", "reason": "RESOURCE:MEMORY"}] (one entry; arn in the short form, without the /<cluster>/ segment). ListTasks afterwards reports only the control task.
  Actual (emulator): HTTP 200, failures == [], tasks == one task record:
    AssertionError: RunTask placed [{'clusterArn': 'arn:aws:ecs:us-west-2:000000000000:cluster/sim2real-behav-ecs-cl-...', 'containerInstanceArn': 'arn:aws:ecs:us-west-2:000000000000:container-instance/sim2real-behav-ecs-cl-.../000...01', 'containers': [{... 'name': 'sim2real-app', 'lastStatus': 'PENDING', 'cpu': '128', 'memory': '8192'}], 'desiredStatus': 'RUNNING', 'lastStatus': 'PENDING', 'launchType': 'EC2', 'taskArn': 'arn:aws:ecs:us-west-2:000000000000:task/sim2real-behav-ecs-cl-.../000...02', ...}] for a definition asking 8192 MiB of a 1024 MiB instance
  (Names, ids, account, region and timestamps above are per-run values; the requirement is tasks==[] plus a single RESOURCE:MEMORY failure attributed to the container instance.)

## Steps to Reproduce
1. ecs:CreateCluster -> <cluster>.
2. Register one EC2 container instance into <cluster> whose registeredResources MEMORY is strictly between 128 and 8192 MiB (e.g. a t3.micro, ~1024 MiB).
3. ecs:RegisterTaskDefinition family=<small>, networkMode=bridge, requiresCompatibilities=[EC2], container {cpu:128, memory:128, portMappings:[{containerPort:80, hostPort:0}]}.
4. ecs:RegisterTaskDefinition family=<oversized>, identical except container memory=8192.
5. ecs:RunTask(cluster=<cluster>, taskDefinition=<small>, count=1, launchType=EC2) -> expect 1 task, 0 failures (control; this currently passes).
6. ecs:RunTask(cluster=<cluster>, taskDefinition=<oversized>, count=1, launchType=EC2).
   Expect: 200 with tasks == [] and failures == [{arn: <short container-instance ARN>, reason: "RESOURCE:MEMORY"}].
   Observed: 200 with failures == [] and one PENDING task whose container reports memory '8192', placed on the 1024 MiB instance.
7. ecs:ListTasks(cluster=<cluster>) -> expect only the control task ARN; observed the oversized task as well.
