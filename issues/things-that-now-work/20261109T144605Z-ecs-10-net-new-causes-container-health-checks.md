Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.container-health-check-transitions-healthy-and-unhealthy ecs.health-check-start-period-suppresses-unhealthy-then-expires ecs.container-depends-on-healthy-waits-for-the-health-check-to-pass ecs.eni-attachment-reaches-attached ecs.submit-attachment-state-change-updates-attachment-status ecs.submit-attachment-state-changes-updates-eni-status ecs.submit-container-state-change-updates-container-record ecs.task-state-change-server-enforced ecs.list-services-by-namespace ecs.task-protection-ttl-expiry ecs.task-protection-enforced-during-scale-in ecs.capacity-provider-removal-while-in-use-rejected ecs.service-cpu-utilization-metric-published ecs.deep-ecs-2-ec2place-draining-instance-relocates-service-task-to-other-host ecs.deep-ecs-3-deploy-fail-metric-alarm-rolls-back-to-rev1-quiet-twin-completes
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs.container-health-check-transitions-healthy-and-unhealthy|_poll_until#16|AssertionError|raise AssertionError(f"timed out waiting for {description!r} (last={last!r})")
Issue-Key: ecs.health-check-start-period-suppresses-unhealthy-then-expires|_poll_until#14|AssertionError|raise AssertionError(
Issue-Key: ecs.container-depends-on-healthy-waits-for-the-health-check-to-pass|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Key: ecs|SubmitAttachmentStateChanges|AccessDeniedException
Issue-Key: ecs|SubmitContainerStateChange|AccessDeniedException
Issue-Key: ecs.task-state-change-server-enforced|_poll_until#23|Timeout|time.sleep(interval_s)
Issue-Key: ecs.list-services-by-namespace|run#26|AssertionError|assert error == expected, f"expected {expected!r}, got {error!r}"
Issue-Key: ecs.task-protection-ttl-expiry|run#22|AssertionError|assert error == expected, f"expected {expected!r}, got {error!r}"
Issue-Key: ecs.task-protection-enforced-during-scale-in|run#33|AssertionError|assert final_states[protected_arn]["lastStatus"] == "RUNNING", (
Issue-Key: ecs.capacity-provider-removal-while-in-use-rejected|_error#15|AssertionError|raise AssertionError(f"expected this call to be refused, but it SUCCEEDED and returned {result!r}")
Issue-Key: ecs.service-cpu-utilization-metric-published|run#15|AssertionError|raise AssertionError(
Issue-Key: ecs.deep-ecs-2-ec2place-draining-instance-relocates-service-task-to-other-host|_registered#2|Timeout|return {}
Issue-Key: ecs.deep-ecs-3-deploy-fail-metric-alarm-rolls-back-to-rev1-quiet-twin-completes|_poll_until#25|Timeout|time.sleep(interval_s)
Issue-Commit: 5252be52e0cc9f5d73bee5aa197523afb271707f

# ecs: 10 net-new causes — container health checks never run, agent Submit*StateChange calls are refused for Fargate tasks, task protection is not honoured, an in-use capacity provider can be removed, and 4 more

> _Note: this issue was identified against the code base at commit `5252be52e0cc9f5d73bee5aa197523afb271707f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTasks

## Description
Cause 1 of 10: a declared container healthCheck is never executed, so Container.healthStatus/Task.healthStatus stay UNKNOWN forever.
AWS: Smithy (ecs) ContainerDefinition.healthCheck (command, interval, timeout, retries, startPeriod) is run by the agent and DescribeTasks reports healthStatus in {HEALTHY, UNHEALTHY, UNKNOWN}. A succeeding probe reaches HEALTHY; a failing one reaches UNHEALTHY after `retries`, and startPeriod only suppresses UNHEALTHY until it expires — it does not suppress forever.
Emulator: `AssertionError: timed out waiting for 'the passing container health check drives the container to HEALTHY' (last=... 'healthStatus': 'UNKNOWN' ...)`; `AssertionError: timed out after 420.0s waiting for both tasks eventually report healthStatus UNHEALTHY ...` (also UNKNOWN); and the dependsOn=HEALTHY gate check is killed at 900s because the condition can never be satisfied.
Why: health is what ECS uses to replace tasks, gate dependsOn, judge deployments and feed load-balancer target health. Pinned at UNKNOWN, a failing container is indistinguishable from an unprobed one and no health-driven behaviour can be tested.
Fix: the ECS container health-check executor and the task/container state it writes (ECS task state machine + describe_tasks serializer), honouring startPeriod and retries.
Traces: ecs.container-health-check-transitions-healthy-and-unhealthy, ecs.health-check-start-period-suppresses-unhealthy-then-expires, ecs.container-depends-on-healthy-waits-for-the-health-check-to-pass.

Cause 2 of 10: the agent-plane Submit*StateChange operations refuse every Fargate task with AccessDeniedException.
AWS: Smithy (ecs) SubmitAttachmentStateChanges (input cluster, attachments; output acknowledgment) and SubmitContainerStateChange (cluster, task, containerName, status, ...) model AccessDeniedException(403), ClientException, ClusterNotFoundException, InvalidParameterException and ServerException. A caller permitted to report a state change gets 200 with an `acknowledgment`; the attachment then reaches ATTACHED and the container record reflects the reported status.
Emulator: `botocore.errorfactory.AccessDeniedException: An error occurred (AccessDeniedException) when calling the SubmitAttachmentStateChanges operation: Fargate task state changes from the agent must have the instance verified` and the same message for SubmitContainerStateChange. That message is emulator-authored — it is not AWS's wording for this error class — and it refuses the call unconditionally for Fargate tasks, so the ENI attachment never reaches ATTACHED. The related server-enforced task-state-change check never reaches a verdict either (`Error: Check timed out after 180.0s`).
Why: the agent plane is unusable: a client (or the emulator's own simulated agent) cannot drive attachment, container or task state, which is why task/ENI lifecycle never advances. The refusal is also unexplainable — a 403 with invented text sends clients hunting for a permission problem that does not exist.
Fix: the submit_attachment_state_changes / submit_container_state_change / submit_task_state_change handlers and the instance-verification gate in front of them — accept agent-reported state changes for Fargate tasks and return `acknowledgment`, reserving AccessDeniedException for a genuinely unauthorized caller with AWS's wording.
Traces: ecs.eni-attachment-reaches-attached, ecs.submit-attachment-state-change-updates-attachment-status, ecs.submit-attachment-state-changes-updates-eni-status, ecs.submit-container-state-change-updates-container-record, ecs.task-state-change-server-enforced.

Cause 3 of 10: ListServicesByNamespace's NamespaceNotFoundException message is emulator-authored.
AWS: Smithy (ecs) ListServicesByNamespace models NamespaceNotFoundException (HTTP 400) for a namespace that does not exist. AWS's message for this class names the namespace from the request: `Failed to retrieve namespace for <namespace>` — the fixed text is `Failed to retrieve namespace for `, the variable part is the `namespace` value the caller passed.
Emulator: code is right but the message is wrong — got `The specified namespace wasn't found.`, which names nothing from the request.
Why: clients and operators match on and log these messages; a generic sentence drops the one piece of information (which namespace) that makes the error actionable, and breaks any assertion or parser written against AWS.
Fix: the list_services_by_namespace handler's NamespaceNotFoundException construction in the ECS crate. (The sibling check list-services-by-namespace-filters-and-validates is owned by 20261104T062603Z-ecs-refusal-messages-and-error-classes-diverge-from.md, which does not carry this check.)
Traces: ecs.list-services-by-namespace.

Cause 4 of 10: a task-protection call whose cluster does not match the task's cluster is refused with the wrong error class and message.
AWS: Smithy (ecs) GetTaskProtection/UpdateTaskProtection model both InvalidParameterException and ClusterNotFoundException. When the cluster exists but the task ARNs belong to a different cluster, AWS refuses with InvalidParameterException and the message `Invalid identifier: cluster identifiers mismatch` — fixed text, no request values.
Emulator: `('ClusterNotFoundException', 'Cluster not found.')` — the wrong error class and a message that misattributes the problem to a missing cluster.
Why: a client cannot distinguish 'you named the wrong cluster for this task' from 'that cluster does not exist'; retry/creation logic keyed on ClusterNotFoundException takes the wrong branch and may try to create a cluster that already exists.
Fix: identifier validation in the get_task_protection/update_task_protection handlers — compare the task ARNs' cluster against the request's cluster and raise InvalidParameterException with AWS's wording.
Traces: ecs.task-protection-ttl-expiry.

Cause 5 of 10: a protected task is not excluded from scale-in stop selection.
AWS: recorded behavior only for the selection rule, with the modeled surface being UpdateTaskProtection (protectionEnabled, expiresInMinutes) and GetTaskProtection. A task with protectionEnabled=true must not be chosen for termination when the service scales in; ECS stops an unprotected task instead (and if none is available, the scale-in waits).
Emulator: `AssertionError: the protected task must be excluded from scale-in stop selection, got {...'lastStatus': 'PROVISIONING'...}` — the protected task is selected and stopped anyway.
Why: task protection exists precisely to keep in-flight work from being killed. Honouring the flag only in GetTaskProtection and not in the scale-in path means a client believes its work is safe while the emulator terminates it.
Fix: the ECS service scale-in / task-selection logic must consult each task's protection record (and its expiry) before choosing tasks to stop.
Traces: ecs.task-protection-enforced-during-scale-in.

Cause 6 of 10: removing a capacity provider that a service is still using is accepted.
AWS: Smithy (ecs) PutClusterCapacityProviders models ResourceInUseException (HTTP 400) and UpdateInProgressException; a request that drops a capacity provider still referenced by a service's capacityProviderStrategy is refused with ResourceInUseException.
Emulator: `AssertionError: expected this call to be refused, but it SUCCEEDED and returned {'cluster': {... 'runningTasksCount': 1, 'activeServicesCount': 1, ... 'capacityProviders': [], 'defaultCapacityProviderStrategy': [], 'attachmentsStatus': 'UPDATE_IN_PROGRESS'}}` — HTTP 200, and the cluster is left with no capacity providers while a service is still running against one.
Why: the guardrail that keeps a running service from losing its capacity source is missing, so a client can strand a live service and only find out when placement starts failing.
Fix: the put_cluster_capacity_providers handler must reject a provider list that removes a provider referenced by any service's capacityProviderStrategy (or by the cluster default in use) with ResourceInUseException.
Traces: ecs.capacity-provider-removal-while-in-use-rejected.

Cause 7 of 10: AWS/ECS CPUUtilization is never published for a running service.
AWS: recorded behavior only for the cadence; CloudWatch GetMetricStatistics returns Datapoints for Namespace 'AWS/ECS', MetricName 'CPUUtilization', dimensioned by ClusterName and ServiceName, for a service with running tasks.
Emulator: `AssertionError: AWS/ECS CPUUtilization metric did not land in CloudWatch within the poll budget (real ingestion latency can exceed this); not a hard failure` — no datapoint is ever published.
Why: every ECS alarm and target-tracking scaling policy reads this series; an always-empty namespace makes them untestable.
Fix: the ECS→CloudWatch metric publisher for AWS/ECS service metrics.
Traces: ecs.service-cpu-utilization-metric-published.

Cause 8 of 10: draining a container instance never relocates the service's task to the other host.
AWS: recorded behavior only. UpdateContainerInstancesState(status='DRAINING') makes ECS stop the service-managed tasks on that instance and place replacements on the remaining ACTIVE instance, observable through DescribeTasks/DescribeServices well inside the check's 180s budget.
Emulator: `Error: Check timed out after 180.0s` — nothing relocates and no terminal state is reached.
Why: draining is the standard mechanism for host maintenance and ASG scale-in; with it inert a client cannot verify that taking a host out of service preserves capacity.
Fix: the ECS container-instance DRAINING handling and the service reconciler's replacement placement.
Traces: ecs.deep-ecs-2-ec2place-draining-instance-relocates-service-task-to-other-host.

Cause 9 of 10: an alarm-driven deployment rollback never completes.
AWS: recorded behavior only. A service deployment with deploymentAlarms configured rolls back to the previous service revision when the named alarm goes into ALARM, and DescribeServiceDeployments/DescribeServices report the rollback finishing, while a 'quiet twin' deployment with no alarm breach completes normally.
Emulator: `Error: Check timed out after 900.0s` — neither the rollback nor the quiet twin reaches a terminal state.
Why: alarm-based rollback is the main safety net for ECS rolling deployments; a client gets no evidence that a bad deployment would be reverted.
Fix: the ECS deployment controller's deploymentAlarms evaluation and rollback path, plus the deployment-record state it reports.
Traces: ecs.deep-ecs-3-deploy-fail-metric-alarm-rolls-back-to-rev1-quiet-twin-completes.

Cause 10 of 10 (grouped with Cause 2's symptom set, tracked separately for replay): the server-enforced task-state-change ordering scenario never reaches a verdict, timing out at 180s on the same agent-plane path.
AWS: recorded behavior only. SubmitTaskStateChange must reject an out-of-order transition and DescribeTasks must reflect only the legal sequence; both are observable immediately.
Emulator: `Error: Check timed out after 180.0s` — the scenario cannot even set up a reportable task.
Why: without an enforceable task-state sequence a client cannot rely on lastStatus/desiredStatus ordering.
Fix: the submit_task_state_change handler's transition validation together with the instance-verification gate named in Cause 2.
Traces: ecs.task-state-change-server-enforced.

## Evidence
Cause 1: `timed out waiting for 'the passing container health check drives the container to HEALTHY' (last=... healthStatus 'UNKNOWN')`; `timed out after 420.0s waiting for both tasks eventually report healthStatus UNHEALTHY ...`; dependsOn=HEALTHY check killed at 900s.
Cause 2: `AccessDeniedException: An error occurred (AccessDeniedException) when calling the SubmitAttachmentStateChanges operation: Fargate task state changes from the agent must have the instance verified` (same for SubmitContainerStateChange).
Cause 3: `expected ('NamespaceNotFoundException', 'Failed to retrieve namespace for no-such-namespace-<id>.local'), got ('NamespaceNotFoundException', "The specified namespace wasn't found.")`.
Cause 4: `expected ('InvalidParameterException', 'Invalid identifier: cluster identifiers mismatch'), got ('ClusterNotFoundException', 'Cluster not found.')`.
Cause 5: `the protected task must be excluded from scale-in stop selection, got {... 'lastStatus': 'PROVISIONING' ...}`.
Cause 6: `expected this call to be refused, but it SUCCEEDED and returned {'cluster': {... runningTasksCount 1, activeServicesCount 1, capacityProviders: [] ...}}` (HTTP 200).
Cause 7: `AWS/ECS CPUUtilization metric did not land in CloudWatch within the poll budget`.
Cause 8: `Error: Check timed out after 180.0s`.
Cause 9: `Error: Check timed out after 900.0s`.
Cause 10: `Error: Check timed out after 180.0s`.

## Steps to Reproduce
Cause 1: RegisterTaskDefinition with a container healthCheck (succeeding, then failing, plus a startPeriod variant); run it and poll DescribeTasks containers[].healthStatus.
Cause 2: run a Fargate task and call ecs SubmitAttachmentStateChanges(cluster, attachments=[{attachmentArn, status:'ATTACHED'}]) and ecs SubmitContainerStateChange(cluster, task, containerName, status:'RUNNING').
Cause 3: ecs ListServicesByNamespace(namespace='no-such-namespace-xxxx.local').
Cause 4: run a task in cluster A, then ecs GetTaskProtection(cluster=B, tasks=[task-in-A]).
Cause 5: UpdateTaskProtection(protectionEnabled=true) on one of a service's tasks, then UpdateService to a lower desiredCount and observe which task is stopped.
Cause 6: create a cluster with a capacity provider and a service using it, then PutClusterCapacityProviders with capacityProviders=[] / a list omitting it.
Cause 7: run a service and poll cloudwatch GetMetricStatistics for AWS/ECS CPUUtilization.
Cause 8: with two container instances and a service task on one, UpdateContainerInstancesState(status='DRAINING') on that one and poll DescribeTasks.
Cause 9: create a service with deploymentConfiguration.alarms, breach the alarm during a new deployment and poll DescribeServiceDeployments.
Cause 10: attempt an out-of-order ecs SubmitTaskStateChange and poll DescribeTasks.
