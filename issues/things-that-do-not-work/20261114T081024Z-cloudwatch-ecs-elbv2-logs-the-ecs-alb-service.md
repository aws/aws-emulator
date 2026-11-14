Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ecs__elbv2__logs.deploy-ecs-alb-service-stack-routes-traffic
Issue-Key: cloudwatch__ecs__elbv2__logs.deploy-ecs-alb-service-stack-routes-traffic|_poll_until#16|Timeout|time.sleep(interval_s)
Issue-Commit: 84939cc283904302bdfe02403fef158918b6c804

# cloudwatch__ecs__elbv2__logs: the ECS + ALB service stack never converges to a routed request, so deploy-ecs-alb-service-stack-routes-traffic reaches no verdict

> _Note: this issue was identified against the code base at commit `84939cc283904302bdfe02403fef158918b6c804`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTargetHealth

## Description
Cause 1 of 1: the CloudFormation-deployed ECS-behind-ALB stack never produces a converged, routable estate, so the check that asserts traffic is routed through the ALB to the ECS task is cut off by the harness before any assertion runs.

AWS: after the stack reaches CREATE_COMPLETE, ecs DescribeServices reports the service with runningCount == desiredCount and a PRIMARY deployment at rolloutState COMPLETED (Smithy: com.amazonaws.ecs#Deployment.rolloutState -> DeploymentRolloutState enum COMPLETED|FAILED|IN_PROGRESS, documented to move IN_PROGRESS -> COMPLETED once the service reaches steady state), elbv2 DescribeTargetHealth reports the task's ip target at TargetHealth.State 'healthy', and an HTTP request to the load balancer's DNSName is answered by the task with HTTP 200. That routed 200 is the recorded behaviour the check asserts.

Emulator: no part of that sequence completes. The check reports only `Error: Check timed out after 60.0s` — the harness cap is reached inside the convergence waits, so no HTTP status, error code or body is ever produced.

Why: a client deploying an ALB-fronted ECS service by CloudFormation and then driving traffic at its DNSName gets neither a healthy target nor a served response, and never any refusal either — the whole deploy-and-route workflow has no observable outcome.

Fix: the ECS service scheduler/task state behind DescribeServices and ListTasks, the ELBv2 target registration and health evaluation behind DescribeTargetHealth, and the ALB data-plane forwarding to the registered task.

Related open issues that look similar but do NOT carry this check's replay key: 20261111T095650Z-cloudwatch-ecs-elbv2-logs-a-cloudformation-alb-fargate.md, 20261113T124512Z-cloudwatch-ecs-elbv2-logs-a-cloudformation-stack-of.md, 20261110T220525Z-cloudwatch-ecs-elbv2-logs-an-alb-answers-502-47cb37.md.

Traces: the failing check below.

## Evidence
Expected (AWS / recorded): stack CREATE_COMPLETE; DescribeServices runningCount == desiredCount with PRIMARY deployment rolloutState 'COMPLETED'; DescribeTargetHealth state 'healthy' for the task's ip target; GET http://<LoadBalancerDnsName>/ answers HTTP 200 from the container.

Actual (emulator): `Error: Check timed out after 60.0s` — no convergence signal and no HTTP response at all.

Failing check:
- cloudwatch__ecs__elbv2__logs.deploy-ecs-alb-service-stack-routes-traffic

## Steps to Reproduce
1. CreateStack with an ALB + target group + listener, an ECS cluster, a task definition with an awslogs log group, and an ECS service (desiredCount 1) bound to the target group.
2. Poll DescribeStacks for a terminal StackStatus, then ecs DescribeServices for runningCount == desiredCount / rolloutState COMPLETED, then elbv2 DescribeTargetHealth for a 'healthy' target.
3. None of those predicates is ever satisfied; the check is killed at the harness's 60 s cap before issuing the HTTP request to the ALB DNSName.
