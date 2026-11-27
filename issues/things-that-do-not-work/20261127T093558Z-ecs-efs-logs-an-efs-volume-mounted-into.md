Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs__efs__logs.ecs-efs-volume-shared-across-two-fargate-tasks-and-readonly-mount-write-denied
Issue-Key: ecs__efs__logs.ecs-efs-volume-shared-across-two-fargate-tasks-and-readonly-mount-write-denied|_poll_until#5|Timeout|return last
Issue-Commit: 0e9afbb76f97c880d990543e0ccf2d5bfecf65a1

# ecs__efs__logs: an EFS volume mounted into Fargate tasks never produces a running container, so cross-task sharing, the readOnly-mount EROFS refusal and EFS metering are all unobservable

> _Note: this issue was identified against the code base at commit `0e9afbb76f97c880d990543e0ccf2d5bfecf65a1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunTask

## Description
One root cause, grounded in 1 failing check the suite report lists under `### ecs__efs__logs`, which is marked neither "ALREADY FILED" nor "OWNED by" any open issue.

Cause 1 of 1: Fargate tasks whose task definition declares an `efsVolumeConfiguration` (file system + access point, transit encryption, IAM authorization) and mounts it at `/mnt/shared` never run their container to completion, so none of the behavior that depends on the mount is ever produced.

  AWS: recorded behavior — task A (read-write mountPoint) writes a marker file to the EFS mount, exits 0, and its `awslogs` stream carries its output (`A-SIZE=<len(marker)+1>` from the container's own `wc -c`, a regular writable mode `-rw...`); task B, started only after A reached STOPPED, reads back A's EXACT bytes from the same file system, appends to it, and exits 0, with its own distinct awsvpc ENI and task ARN; task C, whose mountPoint sets `readOnly: true`, still READS the current content but has its append refused by the kernel with EROFS (`Read-only file system`), exits 0, and leaves no trace — task D then reads exactly A's line plus B's appended line. `DescribeFileSystems(FileSystemId)` reports `SizeInBytes.Value > 0` once bytes have been written, and `DescribeAccessPoints(FileSystemId)` lists exactly the one access point that was created. Smithy (com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113): `RunTask` -> {tasks, failures}; `Task.containers[].exitCode`, `Task.startedAt`/`stoppedAt`, `Task.attachments` (the per-task ENI) are the fields the behavior is read from. The model states those shapes; the mount semantics and the `Read-only file system` errno text are the filesystem's/recording's behavior, not the model's.

  Emulator: none of it happens. The check is cut by the harness: `Error: Check timed out after 60.0s`, inside its `_run_and_wait` / log-tailing loop for the very first task. No task reaches a terminal state with an `exitCode`, no log line is delivered, and no EFS metering or access-point read is reached — so a shared mount, a per-task local mount and no mount at all are indistinguishable, and an accepted write through a `readOnly: true` mount would never be caught.

  Why: a client that depends on EFS for cross-task durability gets no answer at all: neither the bytes nor a failure. The over-permissive case matters most — if the emulator later accepts a write through a read-only mount, nothing in the observable surface reports it, so data that AWS would have refused appears to have landed.

  Fix: the Fargate task runtime in the ECS crate — actually executing the container's command, honouring the task definition's `volumes[].efsVolumeConfiguration` and `mountPoints[].readOnly`, driving the task to STOPPED with a real `containers[].exitCode`, and delivering the container's stdout through the `awslogs` driver (services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/run_task.rs, .../src/ops/describe_tasks.rs, .../src/state.rs), plus EFS `SizeInBytes` accounting on the file system the mount writes to.

  Traces: ecs__efs__logs.ecs-efs-volume-shared-across-two-fargate-tasks-and-readonly-mount-write-denied.

## Evidence
Expected (recorded AWS), with an EFS file system + mount target + access point, an exec role, and a task role allowing `elasticfilesystem:ClientMount`/`ClientWrite` on the access point:
  - A: `sh -c 'echo "$MARKER" > /mnt/shared/payload; sync; echo "A-SIZE=$(wc -c < /mnt/shared/payload)"; echo "A-MODE=$(ls -l ... )"; echo A-DONE'` -> `containers[0].exitCode == 0`, log contains `A-SIZE=<len(marker)+1>` and an `A-MODE=-rw...` line;
  - B (started after A's `stoppedAt`): log contains `B-SAW=<marker>` — A's exact bytes — and `exitCode == 0`; A and B have different task ARNs and different ENIs;
  - C (readOnly mountPoint): log contains `C-READ=<marker>|B-APPEND|`, a `C-DENIED-ERR=` line whose text includes `Read-only file system`, no `C-WROTE`, and `exitCode == 0`;
  - D: log contains `D-CONTENT=<marker>|B-APPEND|` — C's refused write left no trace;
  - `efs describe-file-systems --file-system-id <fs>` -> `SizeInBytes.Value > 0`; `efs describe-access-points --file-system-id <fs>` -> exactly the one created access point.

Actual (emulator): `Error: Check timed out after 60.0s` — the first task never reaches a terminal state with an exit code and no log line is ever delivered, so every assertion above is unreached.

(The marker string, file system id, access point id, cluster and log-group names are per-run generated values. The requirements are: the container's command actually runs and exits 0; B reads byte-for-byte what A wrote; a write through `readOnly: true` fails with the kernel's EROFS text `Read-only file system` and leaves the file unchanged; and the file system's `SizeInBytes` reflects written bytes.)

## Steps to Reproduce
1. Create an EFS file system, a mount target in the Fargate subnet with the task security group, and an access point.
2. Create an exec role and a task role granting `elasticfilesystem:ClientMount`/`ClientWrite` scoped to the access point ARN.
3. `ecs register-task-definition` twice (FARGATE/awsvpc, awslogs log configuration) with `volumes[0].efsVolumeConfiguration = {fileSystemId: <fs>, transitEncryption: ENABLED, authorizationConfig: {accessPointId: <ap>, iam: ENABLED}}` and `containerDefinitions[0].mountPoints[0] = {sourceVolume: shared, containerPath: /mnt/shared, readOnly: false}` — and a second revision with `readOnly: true`.
4. `ecs run-task` the read-write revision with command A (write the marker, print its size and mode), wait for STOPPED and tail its awslogs stream. On the emulator the task never reaches STOPPED with an exit code inside the budget.
5. After A is STOPPED, `ecs run-task` the read-write revision with command B (cat the file, append a line) and assert the log shows A's exact bytes.
6. `ecs run-task` the readOnly revision with command C (read, then attempt an append capturing stderr) and assert the read succeeds while the append fails with `Read-only file system`.
7. `ecs run-task` command D and assert the file holds exactly A's and B's lines.
8. `efs describe-file-systems --file-system-id <fs>` -> `SizeInBytes.Value > 0`.
