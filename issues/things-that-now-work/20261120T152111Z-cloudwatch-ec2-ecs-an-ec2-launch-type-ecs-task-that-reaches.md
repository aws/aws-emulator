Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ec2__ecs.obs-ecs-3-reservation-metrics-match-registered-resources-and-pressure-exceeds-100
Issue-Key: cloudwatch__ec2__ecs.obs-ecs-3-reservation-metrics-match-registered-resources-and-pressure-exceeds-100|run#38|AssertionError|assert placed.get("containerInstanceArn") == ci_arn, (
Issue-Commit: f422e37ca967a244b72f2f746d1d1ab79e5db12d

# An EC2-launch-type ECS task that reaches RUNNING carries no containerInstanceArn, so DescribeTasks never says which container instance hosts it

> _Note: this issue was identified against the code base at commit `f422e37ca967a244b72f2f746d1d1ab79e5db12d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTasks

## Description
Cause 1 of 1: a task run through an EC2 capacity provider reaches lastStatus RUNNING but DescribeTasks returns it with no containerInstanceArn member (absent/None), even though the cluster has exactly one ACTIVE container instance that managed scaling launched and the task is reported as running on it. Failing check: cloudwatch__ec2__ecs.obs-ecs-3-reservation-metrics-match-registered-resources-and-pressure-exceeds-100.

AWS: Smithy (ecs) Task.containerInstanceArn — 'The ARN of the container instances that host the task.' For the EC2 launch type (and for a capacity-provider strategy over an Auto Scaling group) a task that has been placed and is RUNNING always reports containerInstanceArn, and it equals one of the ARNs ListContainerInstances(status=ACTIVE) returns for that cluster; it is the only way a client learns which instance holds the task. (It is legitimately absent only for Fargate tasks, which have no container instance.) Recorded behavior: with one ACTIVE container instance in the cluster and one placed task, DescribeTasks reports containerInstanceArn == that instance's ARN.

Emulator: DescribeTasks on the placed task answers lastStatus 'RUNNING' with containerInstanceArn missing — the check fails with 'task A placed on None, not \'arn:aws:ecs:<region>:<account>:container-instance/<cluster>/<instance-id>\'' while ListContainerInstances(cluster, status=ACTIVE) had already returned exactly that one ARN. So placement happened (or is reported as happened) without the task being bound to, or without the binding being serialized onto, a container instance.

Why: a client cannot tell which instance hosts a task. Everything that joins tasks to instances breaks: draining/replacing an instance before termination, per-instance capacity accounting, debugging via DescribeContainerInstances/runningTasksCount, ECS Exec and agent-level troubleshooting, and the AWS/ECS CPUReservation / MemoryReservation and AWS/ECS/ManagedScaling CapacityProviderReservation series, which are derived from the tasks reserved on each instance's registeredResources and therefore cannot be computed at all. A task appearing to run on no instance is a state real ECS never reports for the EC2 launch type.

Fix: the ECS task placement state and the DescribeTasks/RunTask Task serializer in the ecs service crate — placing a task on an EC2 capacity-provider cluster must record the chosen container instance on the task and emit it as Task.containerInstanceArn (and keep DescribeContainerInstances runningTasksCount in step), rather than leaving the member unset. Related but distinct: open issue 20261120T071621Z-cloudwatch-ec2-ecs-2-net-new-causes-an.md lists this check id, but its replay key for it is the earlier 'lastStatus == RUNNING' assertion (EssentialContainerExited); the check now gets past that assertion and fails on the next one, so the missing containerInstanceArn is not covered by it.

Traces: cloudwatch__ec2__ecs.obs-ecs-3-reservation-metrics-match-registered-resources-and-pressure-exceeds-100, at the containerInstanceArn assertion immediately after the task reaches RUNNING.

## Evidence
Expected (AWS): after RunTask through an EC2 capacity provider and the managed-scaling instance registering ACTIVE, DescribeTasks(cluster, tasks=[task]) returns tasks[0].lastStatus == 'RUNNING' and tasks[0].containerInstanceArn == the single ARN returned by ListContainerInstances(cluster=<cluster>, status='ACTIVE') (shape arn:aws:ecs:<region>:<account>:container-instance/<cluster>/<instance-id>).

Actual (replay): AssertionError: task A placed on None, not 'arn:aws:ecs:us-west-2:000000000000:container-instance/<cluster>/<instance-id>' — lastStatus was RUNNING, containerInstanceArn was absent. The cluster name, account and instance id in the expected ARN are per-run values; the invariant is that a RUNNING EC2-launch-type task reports the ARN of the ACTIVE container instance hosting it.

## Steps to Reproduce
1. Create an ECS cluster, a launch template from the ECS-optimized AMI with ECS_CLUSTER user data, and an Auto Scaling group over it.
2. CreateCapacityProvider over that ASG with managedScaling {status ENABLED, targetCapacity 100, minimumScalingStepSize 1, maximumScalingStepSize 1} and associate it to the cluster (PutClusterCapacityProviders).
3. RegisterTaskDefinition(requiresCompatibilities=['EC2'], networkMode='bridge', cpu='256', memory='256', one essential container running a long-lived command) and RunTask it with that capacity-provider strategy.
4. Poll ListContainerInstances(cluster, status='ACTIVE') until one ARN appears.
5. DescribeTasks(cluster, tasks=[task]). Emulator: lastStatus RUNNING with no containerInstanceArn. AWS: containerInstanceArn equals the ARN from step 4.
