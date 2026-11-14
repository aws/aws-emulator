Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.deploy-ecs-capacity-provider-cfn-usable-for-run-task ecs.container-instance-agent-connection-timeout ecs.deregister-container-instance-with-running-tasks-requires-force ecs.e2e-capacity-provider-autoscaling-task-placement ecs.e2e-capacity-provider-driven-scaling-flow ecs.runtask-without-capacity-rejected ecs.start-task-on-draining-container-instance-rejected
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs|RunTask|InvalidParameterException
Issue-Key: ecs.container-instance-agent-connection-timeout|run#17|AssertionError|assert _sample() is True, (
Issue-Key: ecs.deregister-container-instance-with-running-tasks-requires-force|_error_code#15|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded with {result!r}")
Issue-Key: ecs.e2e-capacity-provider-autoscaling-task-placement|run#47|AssertionError|assert task.get("containerInstanceArn") in container_instance_arns
Issue-Key: ecs.e2e-capacity-provider-driven-scaling-flow|_poll_until#43|Timeout|time.sleep(interval_s)
Issue-Key: ecs.runtask-without-capacity-rejected|_poll_until#16|Timeout|time.sleep(interval_s)
Issue-Key: ecs.start-task-on-draining-container-instance-rejected|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Commit: 1d457c6418d8d9996f08d1176214615fe9686e31

# ecs: 5 net-new causes — CFN-associated capacity provider rejected by RunTask, agentConnected flips instantly, force-less DeregisterContainerInstance accepted, task not bound to its container instance, and EC2-capacity scenarios never settle

> _Note: this issue was identified against the code base at commit `1d457c6418d8d9996f08d1176214615fe9686e31`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunTask

## Description
Five net-new root causes in the ECS EC2-capacity / capacity-provider surface, all grounded in failing checks of this replay that no open issue carries.

Cause 1 of 5: a capacity provider that the cluster has been associated with (via a CloudFormation AWS::ECS::ClusterCapacityProviderAssociations / PutClusterCapacityProviders) is still treated as unassociated by RunTask.
AWS: Smithy (ecs) RunTask input carries capacityProviderStrategy (CapacityProviderStrategy) and models InvalidParameterException (HTTP 400). On AWS, once the named provider appears in the cluster's capacityProviders, RunTask with that provider in capacityProviderStrategy is accepted and returns tasks with no failures; the InvalidParameterException message "The specified capacity provider strategy cannot contain a capacity provider that is not associated with the cluster. Associate the capacity provider with the cluster or specify a valid capacity provider and try again." is fixed text reserved for a provider the cluster really does not carry (recorded behavior only for the accepted case).
Emulator: RunTask answers HTTP 400 InvalidParameterException with exactly that not-associated message for a provider the stack created and associated, so the task is never placed.
Why: a client that provisions capacity providers declaratively and then runs tasks against them can never place a task — the association it just made is invisible to the placement path.
Fix: the RunTask handler's capacity-provider-strategy validation and the cluster capacity-provider association state it reads (the association written by PutClusterCapacityProviders / the CFN resource handler must be the same state RunTask consults).
Traces: ecs.deploy-ecs-capacity-provider-cfn-usable-for-run-task.

Cause 2 of 5: a container instance's agentConnected is flipped to false synchronously with the EC2 StopInstances call, with no connection-timeout window.
AWS: Smithy (ecs) ContainerInstance$agentConnected reports whether the ECS agent is connected. The model does not state the timing, so recorded behavior only: on AWS, immediately after the backing EC2 instance is stopped, DescribeContainerInstances still reports agentConnected true and only later (after the agent's connection times out) reports false.
Emulator: the very first sample taken ~10 ms after StopInstances already reports agentConnected false (samples=[(0.0098, False)]).
Why: a client that distinguishes "host stopping" from "agent disconnected" (draining tooling, health diagnostics, agent-connection alarms) sees an instantaneous, state-change-coupled flip that real AWS never produces, so it cannot observe or test the timeout window at all.
Fix: the container-instance state that derives agentConnected — it must be driven by the agent's own connection/heartbeat lifetime, not computed from the EC2 instance state at read time.
Traces: ecs.container-instance-agent-connection-timeout.

Cause 3 of 5: DeregisterContainerInstance WITHOUT force on an instance that still has a non-STOPPED task placed on it is accepted.
AWS: Smithy (ecs) DeregisterContainerInstance input {cluster, containerInstance (required), force}; models InvalidParameterException (HTTP 400). force is documented as the flag that lets the call through while tasks run ("these tasks remain running ... but they're orphaned"). The refusal itself is recorded behavior only: HTTP 400, code InvalidParameterException, message "Found running tasks on the instance." — entirely fixed text, no request- or state-derived parts — and the instance stays status ACTIVE.
Emulator: the force-less call returns HTTP 200 with the full ContainerInstance already at status 'INACTIVE', agentConnected false, while pendingTasksCount is 1. No refusal is raised at all.
Why: the force-less call is the documented safety interlock; a client using it to deregister hosts one at a time silently orphans live work and reads a cluster state AWS would never permit.
Note: this is the same defect as open issue 20261113T061041Z-ecs-deregistercontainerinstance-without-force-succeeds-on-a-container.md, which carries a different replay key (ecs.deregister-container-instance-requires-force) and does not own this check; fixing one fixes both.
Fix: the DeregisterContainerInstance handler's validation — read input.force, look up the tasks placed on the resolved instance and refuse when any is not STOPPED, leaving status/version untouched.
Traces: ecs.deregister-container-instance-with-running-tasks-requires-force.

Cause 4 of 5: a RUNNING task placed through a capacity-provider strategy on an EC2-backed cluster does not report one of that cluster's registered container instances in containerInstanceArn.
AWS: Smithy (ecs) Task$containerInstanceArn is the ARN of the container instance the task runs on; for an EC2/capacity-provider launch it is always one of the ARNs ListContainerInstances(status=ACTIVE) returns for that cluster (recorded behavior only for the binding).
Emulator: after ListContainerInstances reports an ACTIVE instance with agentConnected true and RunTask(capacityProviderStrategy=[...]) places a task that reaches RUNNING, DescribeTasks reports a containerInstanceArn that is not among those ARNs (assertion `task.get("containerInstanceArn") in container_instance_arns` fails).
Why: a client that correlates tasks to hosts (binpacking checks, per-instance diagnostics, draining decisions) cannot resolve the task's host at all — the ARN it is given does not identify any instance in the cluster.
Fix: the task placement state / RunTask response projection — the placed task must record and serialize the ARN of the container instance it was actually placed on.
Traces: ecs.e2e-capacity-provider-autoscaling-task-placement.

Cause 5 of 5: EC2-capacity scenarios reach no verdict — the checks make no observable progress and are cut off at the harness cap.
AWS: recorded behavior only. Each scenario launches an ECS-optimized EC2 instance (or an ASG behind a capacity provider) with ECS_CLUSTER user data, then waits for ListContainerInstances/DescribeContainerInstances to show it and drives RunTask/StartTask against it; on AWS every leg completes well inside the check's own 420-600 s internal budgets.
Emulator: all three checks are killed with "Check timed out after 60.0s" having produced no verdict at all, so neither the refusal they assert (RunTask "No Container Instances were found in your cluster." with zero capacity; StartTask per-instance placement failure on a DRAINING instance) nor the positive control is ever observable.
Why: a client cannot exercise any EC2-launch-type behaviour that depends on real container-instance capacity; the scenarios simply never settle, which is indistinguishable from a hung service.
Fix: the container-instance registration/placement state that these flows wait on (the path that makes a launched EC2 host observable as a cluster container instance and lets StartTask/RunTask resolve it), plus the StartTask/RunTask placement refusals that follow from it.
Traces: ecs.e2e-capacity-provider-driven-scaling-flow, ecs.runtask-without-capacity-rejected, ecs.start-task-on-draining-container-instance-rejected.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the booted emulator (464/847 passed).

[error] ecs.deploy-ecs-capacity-provider-cfn-usable-for-run-task
  botocore.errorfactory.InvalidParameterException: An error occurred (InvalidParameterException) when calling the RunTask operation: The specified capacity provider strategy cannot contain a capacity provider that is not associated with the cluster. Associate the capacity provider with the cluster or specify a valid capacity provider and try again.
  Expected (AWS): HTTP 200 with the placed task, because the stack associated the provider with the cluster. Actual: HTTP 400 InvalidParameterException with the not-associated message.

[mismatch] ecs.container-instance-agent-connection-timeout
  AssertionError: agentConnected flipped to False synchronously with the StopInstances call -- expected a real connection-timeout window, not an instant flip; samples=[(0.009776643000350305, False)]

[mismatch] ecs.deregister-container-instance-with-running-tasks-requires-force
  AssertionError: expected the call to be rejected, but it succeeded with {'containerInstance': {... 'status': 'INACTIVE', 'agentConnected': False, 'runningTasksCount': 0, 'pendingTasksCount': 1 ...}, 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}
  Expected (AWS): HTTP 400, Error.Code 'InvalidParameterException', Error.Message 'Found running tasks on the instance.' (fixed text), instance still ACTIVE.

[mismatch] ecs.e2e-capacity-provider-autoscaling-task-placement
  File "check_ecs_e2e_capacity_provider_autoscaling_task_placement.py", line 164: assert task.get("containerInstanceArn") in container_instance_arns -> AssertionError
  (container_instance_arns came from ListContainerInstances(status=ACTIVE) and DescribeContainerInstances had already confirmed agentConnected true.)

[mismatch] ecs.e2e-capacity-provider-driven-scaling-flow — Error: Check timed out after 60.0s
[mismatch] ecs.runtask-without-capacity-rejected — Error: Check timed out after 60.0s (never reached its asserted refusal: InvalidParameterException 'No Container Instances were found in your cluster.')
[mismatch] ecs.start-task-on-draining-container-instance-rejected — Error: Check timed out after 60.0s

Smithy consulted (services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/smithy-model/com-amazonaws-ecs-amazonec2containerservicev20141113.json): RunTask$capacityProviderStrategy and InvalidParameterException(400); DeregisterContainerInstanceRequest$force documentation; DescribeCapacityProviders output {capacityProviders, failures, nextToken}.

## Steps to Reproduce
1. Clone the repo and replay suite simparity-behavioral-aws-v1; every check id in trace_ids reproduces.
2. Cause 1: CreateCluster; CreateCapacityProvider over an ASG; associate it (PutClusterCapacityProviders or the CFN association resource); RunTask(cluster, taskDefinition, capacityProviderStrategy=[{capacityProvider: <name>, weight: 1}]). Expected HTTP 200 with the task; actual HTTP 400 InvalidParameterException 'not associated with the cluster'.
3. Cause 2: register/launch a container instance, StopInstances on its EC2 host, and immediately poll DescribeContainerInstances: agentConnected must still be true right after the stop and only later become false.
4. Cause 3: StartTask onto an ACTIVE container instance, wait until pendingTasksCount+runningTasksCount >= 1, then DeregisterContainerInstance with no force. Expected HTTP 400 InvalidParameterException 'Found running tasks on the instance.' and status still ACTIVE; actual HTTP 200 and status INACTIVE.
5. Cause 4: with an ACTIVE, agent-connected container instance in the cluster, RunTask with capacityProviderStrategy and poll DescribeTasks to RUNNING; the task's containerInstanceArn must be one of ListContainerInstances(status=ACTIVE).
6. Cause 5: run the three timing-out checks; each must reach a verdict (zero-capacity RunTask refusal, DRAINING StartTask placement failure, capacity-provider-driven scaling) instead of being cut off with no observable progress.
7. Re-replay all ids in trace_ids to confirm the fix.
