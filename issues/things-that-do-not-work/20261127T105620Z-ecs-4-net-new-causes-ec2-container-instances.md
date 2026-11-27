Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.run-task-places-the-copies-that-fit-and-reports-the-rest-as-failures ecs.run-task-refuses-every-copy-no-container-instance-can-hold ecs.run-task-reports-a-failure-per-container-instance-with-its-own-reason ecs.start-task-keys-each-refused-container-instance-to-its-own-reason ecs.start-task-refuses-every-missing-container-instance-with-one-failure-each ecs.placement-constraint-memberof-attribute-filters-candidate-instances ecs.deep-ecs-2-ec2place-binpack-colocates-while-spread-separates ecs.deep-ecs-2-ec2place-bridge-dynamic-host-port-serves-sibling-and-fixed-port-collision-rejected
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs.run-task-places-the-copies-that-fit-and-reports-the-rest-as-failures|_wait_for#3|AssertionError|raise AssertionError(f"{what}: not reached in {budget_s:.0f}s, last read {seen!r}")
Issue-Key: ecs.run-task-refuses-every-copy-no-container-instance-can-hold|_wait_for#3|AssertionError|raise AssertionError(f"{what}: not reached in {budget_s:.0f}s, last read {seen!r}")
Issue-Key: ecs.run-task-reports-a-failure-per-container-instance-with-its-own-reason|_wait_for#3|AssertionError|raise AssertionError(f"{what}: not reached in {budget_s:.0f}s, last read {seen!r}")
Issue-Key: ecs.start-task-keys-each-refused-container-instance-to-its-own-reason|_wait_for#3|AssertionError|raise AssertionError(f"{what}: not reached in {budget_s:.0f}s, last read {seen!r}")
Issue-Key: ecs.start-task-refuses-every-missing-container-instance-with-one-failure-each|_wait_for#3|AssertionError|raise AssertionError(f"{what}: not reached in {budget_s:.0f}s, last read {seen!r}")
Issue-Key: ecs.placement-constraint-memberof-attribute-filters-candidate-instances|run#19|AssertionError|assert refused["tasks"] == [], (
Issue-Key: ecs.deep-ecs-2-ec2place-binpack-colocates-while-spread-separates|run#14|AssertionError|assert created[svc_binpack]["placementStrategy"] == [{"type": "binpack", "field": "MEMORY"}], (
Issue-Key: ecs.deep-ecs-2-ec2place-bridge-dynamic-host-port-serves-sibling-and-fixed-port-collision-rejected|_poll_until#29|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Commit: 96db52f5b4a4b596ab227bd8becc0930845733a7

# ecs: 4 net-new causes — EC2 container instances never register, memberOf placement constraints are ignored, binpack field is case-folded, and a host-port-colliding task never stops

> _Note: this issue was identified against the code base at commit `96db52f5b4a4b596ab227bd8becc0930845733a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunTask

## Description
Four independent net-new causes in ECS task placement on EC2 capacity. Each is a separate work item; close all four before submitting.

Cause 1 of 4: EC2 capacity never becomes a registered container instance, so every EC2-placement check dies before its assertion.
AWS: an EC2 instance launched with the ECS agent and the cluster's config registers itself; ListContainerInstances/DescribeContainerInstances then report it with its ec2InstanceId, ACTIVE status and registered/remaining resources, and RunTask/StartTask can place on it (recorded behavior; Smithy models RegisterContainerInstance and ContainerInstance accordingly).
Emulator: the cluster's container-instance inventory stays empty — the checks' wait for `hosts join the cluster` reads `{}` and never sees any instance id.
Why: no client can model ECS-on-EC2 at all: capacity is invisible, so placement, draining, per-instance failures and capacity-provider behavior can never be reached.
Note: open issue 20261124T100046Z-ecs-ec2-capacity-never-joins-a-cluster-as.md describes the same symptom for a directly launched agent host and an ASG/capacity-provider host; these five checks were not marked as owned by it, so they are filed here and should be closed together with it if the root cause is shared.
Fix: container-instance registration state and the ListContainerInstances/DescribeContainerInstances read path in the ECS service state (services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/state.rs plus the register/list container-instance handlers under src/ops/).
Traces: checks ecs.run-task-places-the-copies-that-fit-and-reports-the-rest-as-failures, ecs.run-task-refuses-every-copy-no-container-instance-can-hold, ecs.run-task-reports-a-failure-per-container-instance-with-its-own-reason, ecs.start-task-keys-each-refused-container-instance-to-its-own-reason, ecs.start-task-refuses-every-missing-container-instance-with-one-failure-each.

Cause 2 of 4: a memberOf placement constraint that matches no container instance is ignored and the task is placed anyway.
AWS: RunTask evaluates placementConstraints of type memberOf against the candidate container instances. When the cluster-query expression (here `attribute:<custom-attribute> == <value>`) matches no instance, RunTask places nothing: `tasks` is empty and the response carries a failure per unsatisfiable candidate instead (recorded behavior; Smithy: RunTaskRequest.placementConstraints -> PlacementConstraints, RunTaskResponse.failures).
Emulator: returns one task placed despite the expression matching no instance.
Why: a client that uses attribute constraints to pin workloads to qualified hosts gets tasks scheduled on unqualified capacity, and never observes the empty-candidates failure it is written to handle.
Fix: placement-constraint (cluster-query expression) evaluation in the RunTask placement path (src/ops/run_task.rs and the placement logic it calls).
Traces: check ecs.placement-constraint-memberof-attribute-filters-candidate-instances.

Cause 3 of 4: the placement-strategy `field` is case-folded on the way back out.
AWS: PlacementStrategy.field is an opaque string echoed back exactly as submitted (Smithy: PlacementStrategy.field is a String with no enum or pattern; only the `type` member is an enum of random/spread/binpack). A service created with placementStrategy [{type: binpack, field: MEMORY}] describes/echoes `field` with the same spelling the request used.
Emulator: lowercases it — the created service echoes [{'type': 'binpack', 'field': 'memory'}] for a request that sent `MEMORY`.
Why: clients that round-trip a service definition (CDK/CloudFormation drift comparison, idempotent re-apply, config diffing) see a spurious difference between what they submitted and what the API reports, and re-apply or report drift forever.
Fix: the serializer/state write for PlacementStrategy in the CreateService/UpdateService path (src/ops/create_service.rs and the shared placement-strategy conversion) — store and echo the submitted string verbatim.
Traces: check ecs.deep-ecs-2-ec2place-binpack-colocates-while-spread-separates.

Cause 4 of 4: a bridge-network task that cannot get its fixed host port never reaches STOPPED.
AWS: with networkMode bridge, a task whose containerDefinition requests a fixed hostPort already bound on the chosen container instance is refused placement / stops: the task reaches lastStatus STOPPED with its stop reason, while the sibling task using a dynamic host port keeps serving on its assigned ephemeral port (recorded behavior).
Emulator: the task stays non-terminal — the check's wait for the task ARN to reach STOPPED never completes within its budget.
Why: a client waiting on tasks_stopped (or reconciling desired vs. last status) hangs on a task AWS would have terminated, so host-port contention is undetectable.
Fix: host-port conflict handling in the bridge-mode placement/task lifecycle state (src/ops/run_task.rs and the task state machine in src/state.rs) so the losing task transitions to STOPPED.
Traces: check ecs.deep-ecs-2-ec2place-bridge-dynamic-host-port-serves-sibling-and-fixed-port-collision-rejected.

## Evidence
Cause 1 — AssertionError: hosts join the cluster: not reached in 1s, last read {} (raised from _join_hosts/_wait_for in all five placement checks); expected the launched EC2 instance ids to appear as registered container instances of the cluster.
Cause 2 — AssertionError: ECS must place NO task when the memberOf expression 'attribute:sim2real-placement-slot == alpha' matches no container instance, but it returned 1 task(s): ['arn:aws:ecs:<region>:<account>:task/<cluster>/<task-id>']; expected tasks == [] plus a failure entry.
Cause 3 — AssertionError: binpack service must echo its strategy, got [{'type': 'binpack', 'field': 'memory'}]; expected [{'type': 'binpack', 'field': 'MEMORY'}] (the spelling submitted).
Cause 4 — TimeoutError: tasks ['arn:aws:ecs:<region>:<account>:task/<cluster>/<task-id>'] to reach STOPPED: the run budget ended first; expected the fixed-host-port task to reach lastStatus STOPPED while the dynamic-port sibling kept serving.

## Steps to Reproduce
Replay the behavioral suite and re-run the listed checks:
1. Any of ecs.run-task-places-the-copies-that-fit-and-reports-the-rest-as-failures, ecs.run-task-refuses-every-copy-no-container-instance-can-hold, ecs.run-task-reports-a-failure-per-container-instance-with-its-own-reason, ecs.start-task-keys-each-refused-container-instance-to-its-own-reason, ecs.start-task-refuses-every-missing-container-instance-with-one-failure-each — create a cluster, launch EC2 hosts with the ECS agent/instance profile, then poll ListContainerInstances/DescribeContainerInstances for the launched instance ids.
2. ecs.placement-constraint-memberof-attribute-filters-candidate-instances — RunTask with placementConstraints [{type: 'memberOf', expression: 'attribute:<custom> == <value>'}] where no instance carries the attribute.
3. ecs.deep-ecs-2-ec2place-binpack-colocates-while-spread-separates — CreateService with placementStrategy [{'type':'binpack','field':'MEMORY'}] and compare the echoed strategy.
4. ecs.deep-ecs-2-ec2place-bridge-dynamic-host-port-serves-sibling-and-fixed-port-collision-rejected — in bridge mode, run one task with a dynamic hostPort and a second requesting the already-bound fixed hostPort, then wait for the second task to reach STOPPED.
