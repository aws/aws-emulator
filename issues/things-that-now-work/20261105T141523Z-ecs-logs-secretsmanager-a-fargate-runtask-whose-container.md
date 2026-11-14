Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs__logs__secretsmanager.ecs-secret-injection-needs-execution-role-not-task-role
Issue-Key: ecs__logs__secretsmanager.ecs-secret-injection-needs-execution-role-not-task-role|_poll_until#17|Timeout|time.sleep(interval_s)
Issue-Commit: 68ec0c9cae77cfbc133f0186a4c923306bef5402

# ecs__logs__secretsmanager: a Fargate RunTask whose container `secrets` valueFrom names a Secrets Manager secret never reaches a terminal lastStatus and never produces the awslogs stream, so the execution-role-vs-task-role secret-injection check hangs to the harness cap

> _Note: this issue was identified against the code base at commit `68ec0c9cae77cfbc133f0186a4c923306bef5402`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunTask

## Description
One cause.

Cause 1 of 1: a task launched with `launchType=FARGATE` from a task definition whose container carries `secrets` entries (valueFrom = a Secrets Manager secret ARN) plus an `awslogs` logConfiguration never settles. The check polls DescribeTasks for a terminal lastStatus and logs:DescribeLogStreams/GetLogEvents for the container's output, and neither ever changes, so the check runs out at the harness's 60 s cap with no verdict at all. Failing check id: ecs__logs__secretsmanager.ecs-secret-injection-needs-execution-role-not-task-role.

AWS: Smithy com.amazonaws.ecs (AmazonEC2ContainerServiceV20141113) models ContainerDefinition$secrets as a list of Secret { name (required), valueFrom (required) — "The secret to expose to the container. The supported values are either the full ARN of the Secrets Manager secret or the full ARN of the parameter in the SSM Parameter Store" }. Task carries lastStatus, desiredStatus, stopCode (TaskStopCode), stoppedReason, startedAt/stoppingAt/stoppedAt and containers[].exitCode. The agent resolves every `secrets` valueFrom with the task definition's EXECUTION role, before any container exists. Therefore: (a) when only the TASK role carries secretsmanager:GetSecretValue for that secret, the task is failed at start — DescribeTasks reports a terminal lastStatus STOPPED with stopCode "TaskFailedToStart" and a stoppedReason whose fixed text is the ResourceInitializationError class, i.e. of the form "ResourceInitializationError: unable to pull secrets or registry auth: execution resource retrieval failed: unable to retrieve secret from asm: service call has been retried N time(s): AccessDeniedException: User: <execution-role-session-arn> is not authorized to perform: secretsmanager:GetSecretValue on resource: <secret-arn> ..." — the variable parts (role session ARN, secret ARN, retry count) come from the request and state, the ResourceInitializationError / AccessDenied-or-"not authorized" wording is the fixed requirement; no container ever runs, so containers[0].exitCode is ABSENT and the configured log group receives NO log stream; (b) when the same statement sits on the EXECUTION role, the task runs, SIM2REAL_SECRET is injected with the SecretString, the container exits 0 (containers[0].exitCode == 0) and an awslogs stream named "<awslogs-stream-prefix>/<container-name>/<task-id>" appears in the configured log group carrying the printed value. Either way the task reaches a terminal lastStatus in bounded time.

Emulator: neither outcome is ever reached. The replay reports only "Error: Check timed out after 60.0s": after RunTask the task's lastStatus never advances to a terminal value, no stopCode/stoppedReason is ever published, and no log stream is ever created in the awslogs log group, so neither the positive injection leg nor the ResourceInitializationError rejection leg can be graded. The execution-role-versus-task-role distinction is therefore unobservable. Hypothesis: RunTask records a task that stays at its initial lastStatus forever and no container/awslogs side effect is simulated; `secrets` resolution is never attributed to the execution role.

Related but NOT owning this failure: open issue 20261104T044754Z-ecs-logs-secretsmanager-a-task-whose-container-secrets.md describes the same family of never-settling secret-injection tasks, and the replay report marked its sibling check (ecs-secret-json-key-selector-injects-only-that-key) as OWNED by it — but it did NOT mark this check as owned (the recorded replay key for this check no longer matches where it now hangs), so this check is reported separately here rather than silently dropped. A fixer should close both together if one change settles the task lifecycle and the execution-role secret resolution.

Why: a client cannot use ECS secret injection against this emulator at all. A RunTask never becomes observably RUNNING or STOPPED, so any waiter or poll loop (including the AWS CLI's `ecs wait tasks-stopped` and the SDK waiters) hangs; the injected secret value never reaches the container's awslogs stream; and a task definition that AWS refuses at start because the grant sits on the task role instead of the execution role is indistinguishable from one that works — exactly the misconfiguration a client would use the emulator to catch.

Fix: the ECS task lifecycle in the RunTask handler and the task state module, plus DescribeTasks serialization — a task must advance through PENDING to a terminal STOPPED lastStatus and publish stopCode/stoppedReason/stoppedAt and containers[].exitCode (absent when no container started); and the container `secrets` resolution step must authorize against the task definition's executionRoleArn, emit the ResourceInitializationError stoppedReason with stopCode TaskFailedToStart on refusal, and otherwise create the awslogs stream under the configured group and stream prefix with the injected value.

Traces: ecs__logs__secretsmanager.ecs-secret-injection-needs-execution-role-not-task-role (the sibling ecs-secret-json-key-selector-injects-only-that-key times out identically and is already filed).

## Evidence
From the behavioral replay (suite simparity-behavioral-aws-v1, service group ecs__logs__secretsmanager 0/3, 0%):

[mismatch] ecs__logs__secretsmanager.ecs-secret-injection-needs-execution-role-not-task-role
Error: Check timed out after 60.0s

Check intent (from its docstring): "A secret in valueFrom is fetched with the execution role before the container exists: the grant on the task role fails the task at start with ResourceInitializationError and no stream, and the same statement on the execution role lets the task print the value."

Expected (real AWS):
- broken leg: DescribeTasks -> lastStatus "STOPPED" within the check's 600 s budget, stopCode == "TaskFailedToStart", stoppedReason containing "ResourceInitializationError" and an "AccessDenied"/"not authorized" phrase naming the secret ARN, containers[0] with NO exitCode key, and logs:DescribeLogStreams on the configured log group returning [].
- control leg: lastStatus "STOPPED" with containers[0].exitCode == 0, a log stream "ecs/app/<task-id>" in the log group, and an event whose message contains "injected=<secret value>".

Actual (emulator): the task never leaves its initial lastStatus, so `_await_task` (poll DescribeTasks every 5 s, 600 s budget) and `_await_log_stream`/`_await_log_marker` (300 s budgets) never see any change; the whole check is cut off by the harness cap at 60 s. No stopCode, no stoppedReason, no exitCode and no log stream are ever observed — the emulator produced no terminal task state to compare against.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. ec2: create a VPC with one public subnet, an internet gateway and a default route (the check's _public_vpc helper).
3. logs: CreateLogGroup <lg>; assert DescribeLogStreams(<lg>) returns [].
4. secretsmanager: CreateSecret with a plain SecretString; keep its ARN.
5. iam: create role EXEC trusting ecs-tasks.amazonaws.com with an inline policy allowing only logs:CreateLogStream/logs:PutLogEvents; create role TASK trusting ecs-tasks.amazonaws.com with an inline policy allowing secretsmanager:GetSecretValue on the secret ARN.
6. ecs: CreateCluster; RegisterTaskDefinition (FARGATE, awsvpc, cpu 256, memory 512) with executionRoleArn=EXEC, taskRoleArn=TASK and one container: command ["sh","-c","echo \"injected=$SIM2REAL_SECRET\""], secrets [{name: SIM2REAL_SECRET, valueFrom: <secret-arn>}], logConfiguration awslogs -> <lg> with stream prefix "ecs".
7. RunTask (launchType FARGATE, assignPublicIp ENABLED), then poll DescribeTasks for lastStatus == "STOPPED". Expect STOPPED with stopCode "TaskFailedToStart", a ResourceInitializationError/AccessDenied stoppedReason naming the secret ARN, no containers[0].exitCode, and DescribeLogStreams(<lg>) still []. Observed: lastStatus never reaches a terminal value and nothing is ever written to the log group, so the poll never terminates.
8. Repeat with the secretsmanager:GetSecretValue statement moved onto the EXECUTION role. Expect lastStatus "STOPPED" with containers[0].exitCode == 0 and a log stream "ecs/app/<task-id>" containing "injected=<secret value>". Observed: same non-progressing task and no stream.
9. Replay the check in trace_ids.
