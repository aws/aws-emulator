Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs__elbv2__logs.alb-unhealthy-only-target-fails-open-empty-group-returns-503
Issue-Key: ecs__elbv2__logs.alb-unhealthy-only-target-fails-open-empty-group-returns-503|_assert_stays#30|Timeout|observed = probe()
Issue-Commit: 0e9afbb76f97c880d990543e0ccf2d5bfecf65a1

# ecs__elbv2__logs: the ALB fail-open / empty-target-group 503 lifecycle never converges — no RUNNING Fargate task, no awslogs stream and no unhealthy target health are observed

> _Note: this issue was identified against the code base at commit `0e9afbb76f97c880d990543e0ccf2d5bfecf65a1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTargetHealth

## Description
One root cause, grounded in 1 failing check the suite report lists under `### ecs__elbv2__logs`, which is marked neither "ALREADY FILED" nor "OWNED by" any open issue.

Cause 1 of 1: the chain this behavior is read from — a standalone Fargate task serving HTTP on port 80, its `awslogs` stream, and an ALB target registered by the task's private IP whose health check points at a path the container 404s — never advances, so neither the documented fail-open routing nor the empty-target-group 503 is ever produced.

  AWS: recorded behavior, with the ELBv2 documented health-check semantics (https://docs.aws.amazon.com/elasticloadbalancing/latest/application/target-group-health-checks.html) — `RunTask` places the task, `DescribeTasks` reports `lastStatus: RUNNING` and an attachment carrying the task's `privateIPv4Address`; the container's start line appears in its `awslogs` stream `ecs/app/<task-id>`; after `RegisterTargets(TargetGroupArn, [{Id: <task ip>, Port: 80}])` with `HealthCheckPath` pointing at a path the container answers 404 for, `DescribeTargetHealth` reports that target `TargetHealth.State: "unhealthy"` with `Reason: "Target.ResponseCodeMismatch"` and `Description: "Health checks failed with these codes: [404]"` — the Reason and the fixed part of the Description are AWS's own text, the `[404]` list being the code the health check actually observed. With that single target unhealthy, the ALB FAILS OPEN and still serves HTTP 200 from the container (the body carries the container's marker) while the target keeps reporting unhealthy; a target group with NO registered targets returns HTTP 503; and correcting `HealthCheckPath` restores a `healthy` target with 200s.

  Emulator: the check is cut by the harness: `Error: Check timed out after 60.0s`, inside the early polling (`_await_task` for RUNNING, `_await_log_stream`, then the `target <ip> goes unhealthy` poll). Neither a RUNNING task with a private IP, nor a log line, nor any `DescribeTargetHealth` state transition is produced, so the fail-open behavior, the exact unhealthy Reason/Description, and the empty-group 503 are all unobservable.

  Why: a client cannot distinguish an ALB that fails open (AWS's documented behavior when no target is healthy) from one that 503s, and cannot see WHY a target is unhealthy. Load-balancer troubleshooting that keys off `Target.ResponseCodeMismatch` and the `Health checks failed with these codes: [...]` description gets nothing, and a wrong routing decision (503 instead of fail-open 200, or 200 from an empty group) would go unnoticed.

  Fix: the Fargate task runtime and its ENI/private-IP reporting in the ECS crate (services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/run_task.rs, .../src/ops/describe_tasks.rs, .../src/state.rs) together with the ELBv2 target health evaluation and listener routing that must fail open when no target is healthy and 503 an empty target group (services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/src/ops/describe_target_health.rs and the target-group state it is served from).

  Traces: ecs__elbv2__logs.alb-unhealthy-only-target-fails-open-empty-group-returns-503.

## Evidence
Expected (recorded AWS):
  - `ecs run-task` (FARGATE, awsvpc, container serving :80 and printing `<marker> httpd-start`) -> `describe-tasks` reports `lastStatus: "RUNNING"` and an attachment detail `privateIPv4Address`;
  - the awslogs stream `ecs/app/<task-id>` contains the `<marker> httpd-start` line;
  - `elbv2 register-targets --target-group-arn <tg> --targets Id=<task ip>,Port=80`, with `HealthCheckPath` set to a path the container 404s -> `elbv2 describe-target-health` reports `State: "unhealthy"`, `Reason: "Target.ResponseCodeMismatch"`, `Description: "Health checks failed with these codes: [404]"`;
  - `GET http://<alb-dns>/` returns HTTP 200 with the marker in the body for the whole observation window WHILE that same target reports unhealthy (fail open);
  - a target group with no registered targets returns HTTP 503;
  - fixing `HealthCheckPath` returns the target to `healthy` with continued 200s.

Actual (emulator): `Error: Check timed out after 60.0s` — the body never gets past waiting for the task to be RUNNING, its log stream to appear, and the target to report unhealthy, so no status, Reason or Description is ever produced to compare.

(The marker, task id, private IP, ALB DNS name and ARNs are per-run generated values. The requirements are the `unhealthy` state with Reason `Target.ResponseCodeMismatch` and the Description template `Health checks failed with these codes: [<codes observed>]`, the fail-open 200 while the only target is unhealthy, and the 503 from an empty target group.)

## Steps to Reproduce
1. Create a VPC-default ALB security group open on 80 and a task security group open on 80 from the ALB SG; create a log group and an ECS exec role allowing `logs:CreateLogStream`/`logs:PutLogEvents`.
2. `ecs register-task-definition` (FARGATE/awsvpc, awslogs with stream prefix `ecs`) for a container that serves HTTP 200 with a marker on :80 and prints `<marker> httpd-start`.
3. `ecs run-task` it and poll `ecs describe-tasks` for `lastStatus: RUNNING` plus the attachment's `privateIPv4Address` — on the emulator this never converges inside the budget.
4. Poll `logs get-log-events` on `ecs/app/<task-id>` for the `httpd-start` line.
5. `elbv2 create-target-group --health-check-path /sim2real-no-such-path`, create an ALB and a listener forwarding to it, then `elbv2 register-targets --targets Id=<task ip>,Port=80`.
6. Poll `elbv2 describe-target-health` until the target is `unhealthy`; assert `Reason == "Target.ResponseCodeMismatch"` and `Description == "Health checks failed with these codes: [404]"`.
7. Repeatedly `GET http://<alb-dns>/` while re-reading target health: expect HTTP 200 with the marker (fail open) while the target stays unhealthy.
8. Point the listener at a target group with no registered targets: expect HTTP 503. Then fix `HealthCheckPath` and expect the target to go `healthy` with 200s.
