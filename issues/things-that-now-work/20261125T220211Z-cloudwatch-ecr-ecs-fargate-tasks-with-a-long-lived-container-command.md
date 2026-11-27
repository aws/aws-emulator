Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ecr__ecs.ecr-ecs-fargate-task-runs cloudwatch__ecr__ecs.deploy-cfn-ecs-task-role-trust-policy-correctly-attached cloudwatch__ecr__ecs.deploy-cfn-ecs-service-fails-without-iam-trust-policy-rejected cloudwatch__ecr__ecs.ecs-service-stabilizes-and-logs-to-cloudwatch
Issue-Key: cloudwatch__ecr__ecs.ecr-ecs-fargate-task-runs|_await_task_running#22|AssertionError|assert task.get("lastStatus") == "RUNNING", (
Issue-Key: cloudwatch__ecr__ecs.deploy-cfn-ecs-task-role-trust-policy-correctly-attached|run#21|AssertionError|assert status == "RUNNING", f"task with a correctly-trusted role failed to start (status={status})"
Issue-Key: cloudwatch__ecr__ecs.deploy-cfn-ecs-service-fails-without-iam-trust-policy-rejected|run#22|AssertionError|assert good_status == "RUNNING", f"control task with a correctly-trusted role failed (status={good_status})"
Issue-Key: cloudwatch__ecr__ecs.ecs-service-stabilizes-and-logs-to-cloudwatch|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Commit: 76a27d9bdd16e996b43ee8be0e1161c3fa717030

# Fargate tasks with a long-lived container command stop immediately with "Essential container in task exited" instead of staying RUNNING

> _Note: this issue was identified against the code base at commit `76a27d9bdd16e996b43ee8be0e1161c3fa717030`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTasks

## Description
One net-new root cause, grounded in the failing behavioral checks under the suite heading `cloudwatch__ecr__ecs`.

Cause 1 of 1: every launched Fargate task is driven to STOPPED within seconds, regardless of whether its container command is long-lived, so no task or service ever presents a RUNNING positive control.
AWS: Smithy: ecs#Task.lastStatus is "The last known status for the task" (Task Lifecycle) and ecs#Task.stoppedReason is "The reason that the task was stopped"; a task whose essential container runs a long-lived command (a server image, or `sleep <n>`) walks PROVISIONING -> PENDING -> RUNNING and REMAINS at lastStatus=RUNNING with startedAt set and no stoppedAt/stopCode/stoppedReason until it is stopped or its command exits. The fixed text "Essential container in task exited" is reserved for a task whose essential container actually terminated, and is paired with the container's real exitCode. Recorded behavior only for the transition latency, which the checks poll for.
Emulator: DescribeTasks reports the task at lastStatus=STOPPED with stoppedReason "Essential container in task exited", desiredStatus=STOPPED, the container at lastStatus=STOPPED exitCode=0 and the task's ENI attachment already DELETED, within seconds of RunTask/CreateService — even for task definitions whose command is long-lived. Consequently a service built on such a task definition never stabilizes (its check exhausts its budget polling for a stable/running service).
Why: the RUNNING state is unobservable, so every positive control is impossible: waiters on tasks_running and services_stable never succeed, awslogs stream and in-container side effects never happen, negative tests that distinguish "task failed to start" from "task started fine" lose their control arm, and a client reading stoppedReason is told its healthy workload exited when it did not.
Fix: the ECS task lifecycle must derive a task's terminal transition from the container command's lifetime (keep a task at RUNNING until it is stopped or its command genuinely exits) and must only set stoppedReason "Essential container in task exited"/exitCode for a container that terminated (services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src — the task state machine behind RunTask/DescribeTasks and the service scheduler). Note: the open issue 20261104T060239Z-cloudwatch-ecr-ecs-fargate-tasks-never-leave-running.md describes the OPPOSITE symptom for this heading (tasks parked at RUNNING and never reaching a terminal state) and lists cloudwatch__ecr__ecs.ecs-service-stabilizes-and-logs-to-cloudwatch among its traces; the replay no longer matches its key for these checks, and the current behavior — immediate STOPPED with a false "Essential container in task exited" — is the inverse defect, so it is reported here.
Traces: cloudwatch__ecr__ecs.ecr-ecs-fargate-task-runs, cloudwatch__ecr__ecs.deploy-cfn-ecs-task-role-trust-policy-correctly-attached, cloudwatch__ecr__ecs.deploy-cfn-ecs-service-fails-without-iam-trust-policy-rejected, cloudwatch__ecr__ecs.ecs-service-stabilizes-and-logs-to-cloudwatch.

## Evidence
ecr-ecs-fargate-task-runs:
AssertionError: task did not reach RUNNING: lastStatus=STOPPED stoppedReason=Essential container in task exited

deploy-cfn-ecs-task-role-trust-policy-correctly-attached:
AssertionError: task with a correctly-trusted role failed to start (status=STOPPED)

deploy-cfn-ecs-service-fails-without-iam-trust-policy-rejected:
AssertionError: control task with a correctly-trusted role failed (status=STOPPED)  — the negative arm (missing trust) is graded against this control, which must be RUNNING.

ecs-service-stabilizes-and-logs-to-cloudwatch:
Error: Check timed out after 60.0s while polling for the service to stabilize on tasks that never stay RUNNING.

Expected (AWS): DescribeTasks lastStatus=RUNNING, startedAt set, no stoppedAt/stopCode/stoppedReason, container lastStatus=RUNNING with no exitCode.
Actual (emulator): lastStatus=STOPPED, desiredStatus=STOPPED, stoppedReason="Essential container in task exited", container exitCode=0, ENI attachment status DELETED, seconds after launch.

## Steps to Reproduce
1. Create an ECS cluster; create an IAM role trusting ecs-tasks.amazonaws.com with AmazonECSTaskExecutionRolePolicy; RegisterTaskDefinition (FARGATE, awsvpc, cpu 256 / memory 512, image public.ecr.aws/docker/library/busybox:latest, command ["sleep","1800"] or any long-lived entrypoint, awslogs driver).
2. RunTask and poll DescribeTasks: within seconds lastStatus=STOPPED with stoppedReason "Essential container in task exited" and container exitCode 0 — RUNNING is never observed.
3. CreateService on the same task definition with desiredCount 1 and wait for it to stabilize: it never does, and no awslogs stream is written.
