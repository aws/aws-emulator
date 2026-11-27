Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr__ecs__elbv2.ecs-task-fails-when-ecr-repo-policy-denies-pull ecr__ecs__elbv2.ecs-task-fails-with-nonexistent-ecr-image-tag ecr__ecs__elbv2.ecs-task-with-foreign-ecr-registry-image-fails-to-pull
Issue-Key: ecr__ecs__elbv2.ecs-task-fails-when-ecr-repo-policy-denies-pull|_run_and_wait#12|Timeout|time.sleep(5)
Issue-Key: ecr__ecs__elbv2.ecs-task-fails-with-nonexistent-ecr-image-tag|_run_and_wait#15|Timeout|time.sleep(5)
Issue-Key: ecr__ecs__elbv2.ecs-task-with-foreign-ecr-registry-image-fails-to-pull|_run_and_wait#16|Timeout|time.sleep(5)
Issue-Commit: a14965d6060559849ea419b74faa25e7836c4558

# ecr__ecs__elbv2: a Fargate task whose ECR image cannot be pulled never stops with TaskFailedToStart/CannotPullContainerError, and no launched task ever reaches STOPPED (2 causes)

> _Note: this issue was identified against the code base at commit `a14965d6060559849ea419b74faa25e7836c4558`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunTask

## Description
Two net-new root causes, both grounded in failing behavioral checks under the suite heading `ecr__ecs__elbv2`. All three checks are "permission-boundary" ECR-pull rejection scenarios and all three die the same way: the harness budget ends while DescribeTasks is still polled for a terminal task ("Check timed out after 60.0s").

Cause 1 of 2: a task launched by RunTask never reaches a terminal state, so DescribeTasks never reports a stopped task.
AWS: Smithy: ecs#Task.lastStatus is "The last known status for the task" (Task Lifecycle), and a task that terminates also carries stoppingAt/stoppedAt/executionStoppedAt, stopCode (Smithy: ecs#TaskStopCode = TaskFailedToStart | EssentialContainerExited | UserInitiated | ServiceSchedulerInitiated | SpotInterruption | TerminationNotice | InfrastructureHealth), stoppedReason and per-container reason/exitCode, plus pullStartedAt/pullStoppedAt for the image pull. A Fargate task walks PROVISIONING -> PENDING -> RUNNING -> DEACTIVATING -> STOPPING -> STOPPED (or PROVISIONING/PENDING -> STOPPED when the image pull fails), so a client polling DescribeTasks observes lastStatus=STOPPED within the launch window; recorded behavior only for the timing the suite polls for.
Emulator: DescribeTasks keeps answering a non-terminal lastStatus for the whole poll window, for both the negative task and the positive-control task. Every one of the three checks exhausts the 60s harness budget inside the describe_tasks poll loop that exits only on lastStatus == "STOPPED"; no stoppedAt, stopCode, stoppedReason or container reason is ever produced.
Why: a client cannot observe the outcome of any task it launches - describe_tasks/tasks_stopped waiters, exit-code assertions and stoppedReason diagnosis never complete, so every end-to-end ECS task workflow hangs instead of finishing, and a failing launch is indistinguishable from a healthy one.
Fix: the ECS task state machine behind RunTask/DescribeTasks and the stored task records (services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src) must drive a launched task through to STOPPED and populate the stopped fields. The open issue 20261104T060239Z-cloudwatch-ecr-ecs-fargate-tasks-never-leave-running.md describes the same symptom under the heading `cloudwatch__ecr__ecs` and does not carry these three checks.
Traces: ecr__ecs__elbv2.ecs-task-fails-when-ecr-repo-policy-denies-pull, ecr__ecs__elbv2.ecs-task-fails-with-nonexistent-ecr-image-tag, ecr__ecs__elbv2.ecs-task-with-foreign-ecr-registry-image-fails-to-pull.

Cause 2 of 2: ECS never resolves or authorizes the container image against ECR at task start, so an unpullable image produces no pull failure.
AWS: when the ECS agent/Fargate cannot pull a container definition's image, the task is stopped with stopCode=TaskFailedToStart (Smithy: ecs#TaskStopCode) and the container's reason / the task's stoppedReason names the pull error class CannotPullContainerError ("Stopped tasks error codes" in the ECS Developer Guide), while an identical task on a pullable image of the same repository with the same execution role stops with no pull error. The fixed text of the diagnosis is the error class `CannotPullContainerError`; the rest of the reason is variable - the image reference from the request's task definition, the registry host and the underlying pull diagnosis (repository/tag not found, or the authorization failure) taken from the registry's answer. This must hold for each of the three pull-failure predicates the checks exercise: (a) the ECR repository policy Denies ecr:BatchGetImage/ecr:GetDownloadUrlForLayer to the task's executionRoleArn; (b) the image tag does not exist in an otherwise pullable repository; (c) the image names another account's registry host (<other-account-id>.dkr.ecr.<region>.amazonaws.com/...), which the caller cannot read.
Emulator: no pull outcome is ever produced for any of the three predicates - the task is parked non-terminally (Cause 1) with no stopCode and no CannotPullContainerError text, so the deny policy set by SetRepositoryPolicy, the nonexistent tag and the foreign registry host are all indistinguishable from a successful pull. The repository policy and the registry/account of the image reference are not consulted at task start at all.
Why: a client's negative tests for ECR pull permissions and for bad image references silently never fire: a misconfigured execution role, a typo'd tag or a cross-account image looks exactly like a working deployment, and the CannotPullContainerError diagnosis every ECS runbook keys on is unavailable.
Fix: RunTask/the task launch path must resolve the container definition's image against the ECR registry (registry account + repository + tag) and evaluate the repository policy for the task's executionRoleArn, then stop the task with stopCode=TaskFailedToStart and a CannotPullContainerError reason when the pull is refused (services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src, together with the ECR authorization it must call).
Traces: ecr__ecs__elbv2.ecs-task-fails-when-ecr-repo-policy-denies-pull (deny policy), ecr__ecs__elbv2.ecs-task-fails-with-nonexistent-ecr-image-tag (missing tag), ecr__ecs__elbv2.ecs-task-with-foreign-ecr-registry-image-fails-to-pull (foreign registry).

## Evidence
All three checks report: "Error: Check timed out after 60.0s".

Each check pushes a minimal image to a fresh ECR repository (PutImage with a docker v2 manifest), creates an execution role trusting ecs-tasks.amazonaws.com with AmazonECSTaskExecutionRolePolicy, creates a cluster and registers a FARGATE/awsvpc task definition (cpu 256, memory 512), then runs the task and polls:
    last = ecs.describe_tasks(cluster=cluster, tasks=[task_arn])["tasks"][0]
    if last["lastStatus"] == "STOPPED": return last
    time.sleep(5)
The loop never exits, so the 60s harness budget ends first and the assertions below are never reached.

Expected (AWS), per check:
- ecs-task-fails-when-ecr-repo-policy-denies-pull: after SetRepositoryPolicy with Effect=Deny on ecr:BatchGetImage/ecr:GetDownloadUrlForLayer for the execution role, DescribeTasks reports stopCode == "TaskFailedToStart" and the stoppedReason/container reason contains "CannotPullContainerError"; before the policy is set and after DeleteRepositoryPolicy, the same task shows no CannotPullContainerError.
- ecs-task-fails-with-nonexistent-ecr-image-tag: the task on <repositoryUri>:does-not-exist-tag reports stopCode == "TaskFailedToStart" with "CannotPullContainerError"; the task on <repositoryUri>:v1 shows none.
- ecs-task-with-foreign-ecr-registry-image-fails-to-pull: the task on <other-account-id>.dkr.ecr.<region>.amazonaws.com/<repo>:v1 reports stopCode == "TaskFailedToStart" with "CannotPullContainerError"; the task on the caller's own repositoryUri shows none.

Actual (emulator): lastStatus never becomes STOPPED for either the negative or the positive task, and no stopCode / stoppedReason / container reason is ever populated, so neither the terminal state nor the pull diagnosis exists.

## Steps to Reproduce
1. CreateRepository in ECR and PutImage a minimal docker v2 manifest under tag v1.
2. CreateRole trusting ecs-tasks.amazonaws.com, AttachRolePolicy arn:aws:iam::aws:policy/service-role/AmazonECSTaskExecutionRolePolicy.
3. CreateCluster; RegisterTaskDefinition with requiresCompatibilities=[FARGATE], networkMode=awsvpc, cpu=256, memory=512, executionRoleArn=<role>, containerDefinitions=[{name: app, image: <repositoryUri>:v1, essential: true}].
4. RunTask (launchType=FARGATE, awsvpcConfiguration with a subnet + default security group) and poll DescribeTasks: lastStatus never reaches STOPPED and no stopCode/stoppedReason appears (Cause 1).
5. SetRepositoryPolicy on the repository with a Deny of ecr:BatchGetImage and ecr:GetDownloadUrlForLayer for the execution-role ARN, RunTask again: no TaskFailedToStart / CannotPullContainerError is ever reported (Cause 2a).
6. RegisterTaskDefinition with image <repositoryUri>:does-not-exist-tag and RunTask: no TaskFailedToStart / CannotPullContainerError (Cause 2b).
7. RegisterTaskDefinition with image <other-account-id>.dkr.ecr.<region>.amazonaws.com/<repo>:v1 and RunTask: no TaskFailedToStart / CannotPullContainerError (Cause 2c).
