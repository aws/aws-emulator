Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__ecs__servicediscovery.ecs-run-task-is-blocked-by-insufficient-memory-on-the-only-container-instance ec2__ecs__servicediscovery.ecs-on-ec2-places-a-task-that-ecs-auto-registers-in-cloud-map
Issue-Key: ec2__ecs__servicediscovery.ecs-run-task-is-blocked-by-insufficient-memory-on-the-only-container-instance|run#16|AssertionError|assert refused["tasks"] == [], (
Issue-Key: ec2__ecs__servicediscovery.ecs-on-ec2-places-a-task-that-ecs-auto-registers-in-cloud-map|_poll#21|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Commit: 6ec43c73f71e6013077e783d53ce3ba5d179d34d

# ecs on ec2: RunTask places a task that asks more memory than the container instance has instead of returning a RESOURCE:MEMORY failure, and a running task is never auto-registered in Cloud Map (2 causes)

> _Note: this issue was identified against the code base at commit `6ec43c73f71e6013077e783d53ce3ba5d179d34d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunTask

## Description
Two net-new causes on the ECS-on-EC2 path. Neither is carried by an open issue or by a scaffold-stub issue.

Cause 1 of 2: RunTask ignores the container instance's registered resources and places an unplaceable task.
AWS: RunTask does not raise for a placement refusal — it answers 200 with tasks=[] and exactly one entry in failures whose arn is the container instance that could not take the task and whose reason is the structured code "RESOURCE:MEMORY". Smithy: RunTask output declares both tasks and failures lists; the reason string is recorded behavior.
Emulator: a task definition asking 8192 MiB is placed on the only container instance, which registered 1024 MiB — RunTask returns one task in PENDING on that instance's containerInstanceArn and no failures at all.
Why: the cluster's capacity accounting is fiction. A client that sizes a cluster from RunTask failures (or that treats a returned task as proof capacity existed) sees a task that AWS would never have placed, and never learns the memory it asked for is unavailable.
Fix: the RunTask placement logic — subtract the task definition's cpu/memory from the container instance's remaining registered resources and emit the RESOURCE:MEMORY failure instead of placing (the ECS crate's run_task handler and its container-instance resource state).
Traces: ec2__ecs__servicediscovery.ecs-run-task-is-blocked-by-insufficient-memory-on-the-only-container-instance.

Cause 2 of 2: a task placed by a service with a serviceRegistries entry is never registered in Cloud Map.
AWS: an ECS service declaring serviceRegistries auto-registers each RUNNING task as an instance of the Cloud Map service, so ListInstances / DiscoverInstances returns it once the task runs; the registration is removed when the task stops. Recorded behavior only (the registration is an ECS-side side effect, not a modelled member).
Emulator: the registration never appears. The check polls the EC2 -> ECS-agent -> placement -> Cloud Map chain to its deadline and sees nothing: "ECS auto-registered the running task in Cloud Map: the run budget ended first" — no instance is ever discoverable for the service.
Why: service discovery is the only way the task is addressable in this topology; a client that resolves its backend through Cloud Map gets an empty answer forever, with no error to tell it the integration did not happen.
Fix: the ECS service/task lifecycle component that honours serviceRegistries — on a task reaching RUNNING it must call the servicediscovery registration (and deregister on stop).
Traces: ec2__ecs__servicediscovery.ecs-on-ec2-places-a-task-that-ecs-auto-registers-in-cloud-map.

## Evidence
Cause 1 — AssertionError: RunTask placed [{... 'containerInstanceArn': 'arn:aws:ecs:...:container-instance/<cluster>/<id>', 'lastStatus': 'PENDING', 'containers': [{... 'cpu': '128', 'memory': '8192'}] ...}] for a definition asking 8192 MiB of a 1024 MiB instance. AWS answers tasks=[] with one failures entry: arn = that container instance, reason = "RESOURCE:MEMORY".
Cause 2 — TimeoutError: ECS auto-registered the running task in Cloud Map: the run budget ended first (the Cloud Map service lists no instance for the RUNNING task).

## Steps to Reproduce
1. Launch one EC2 container instance (1024 MiB registered) into a cluster and wait for it to be ACTIVE with its agent connected.
2. RegisterTaskDefinition requesting 8192 MiB and call RunTask(cluster, taskDefinition, count=1, launchType=EC2): the emulator returns a PENDING task instead of tasks=[] plus a RESOURCE:MEMORY failure.
3. Create a Cloud Map namespace/service and an ECS service with serviceRegistries pointing at it, wait for the task to reach RUNNING, then poll servicediscovery ListInstances/DiscoverInstances: no instance is ever registered.
