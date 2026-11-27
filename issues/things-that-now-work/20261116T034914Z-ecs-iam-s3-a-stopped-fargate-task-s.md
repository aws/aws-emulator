Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs__iam__s3.fargate-task-role-missing-s3-putobject-grant-is-denied-then-repaired
Issue-Key: ecs__iam__s3.fargate-task-role-missing-s3-putobject-grant-is-denied-then-repaired|_wait_stopped#22|AssertionError|assert writer.get("exitCode") == 0, (
Issue-Commit: 3b9b0467f2eb3d7399c7e850c9f1cb96948fddb1

# ecs__iam__s3: a stopped Fargate task's container reports no exitCode, so a task role's missing s3:PutObject denial and its repair are never observable

> _Note: this issue was identified against the code base at commit `3b9b0467f2eb3d7399c7e850c9f1cb96948fddb1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTasks

## Description
Cause 1 of 1: a Fargate task whose container stopped is reported with no exit status, and the container's command never ran, so the task-role permission chain (denied, then allowed after the policy is attached) produces no observable result.
AWS: a stopped container carries the process's exit status in `exitCode` (Smithy: ecs#Container.exitCode : BoxedInteger, "The exit code returned from the container"), with `reason` and the task's stoppedReason "Essential container in task exited". The container's process uses the task role's credentials from the task-role credentials endpoint, so a writer whose role lacks s3:PutObject gets AccessDenied from S3 and reports DENIED, and the same writer with the grant attached exits 0 after the PutObject succeeds.
Emulator: DescribeTasks reports the task STOPPED with container exitCode absent/None and stoppedReason 'Essential container in task exited'; the writer never ran, so neither the AccessDenied path nor the repaired ALLOWED path is exercised and no object is written.
Why: a client validating IAM task-role scoping (the standard way to confirm least privilege for a container) gets no signal at all: the denied task and the allowed task are indistinguishable, and automation that gates on the container's exit code never receives a value.
Fix: the RunTask/StartTask task-execution path (running the container with the task role's credentials available to its process) and the container exit reporting DescribeTasks serializes (Container.exitCode / Container.reason).
Traces: ecs__iam__s3.fargate-task-role-missing-s3-putobject-grant-is-denied-then-repaired.

## Evidence
Expected (AWS): after the s3:PutObject grant is attached, DescribeTasks on the stopped writer task reports its container with exitCode 0 (and its log line reports ALLOWED); without the grant it reports DENIED with the S3 AccessDenied error. Actual (emulator): AssertionError: task '<task-id>' container exited None: 'Essential container in task exited' — the container's exitCode is absent/None for both the denied and the repaired run (the task id is a per-run generated value; the absent exitCode is the invariant).

## Steps to Reproduce
1. CreateBucket; create a task role WITHOUT s3:PutObject plus an execution role; CreateCluster and a log group.
2. RegisterTaskDefinition (FARGATE/awsvpc) with a container (public.ecr.aws/aws-cli/aws-cli) whose script retries `aws s3 cp` to the bucket and prints DENIED or ALLOWED, exiting 0 on the expected outcome.
3. RunTask with the un-granted role, poll DescribeTasks to STOPPED, read containers[].exitCode → None.
4. Attach an s3:PutObject policy to the task role, RunTask again, poll to STOPPED → exitCode is still None and no object appears in the bucket.
