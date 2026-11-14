Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ecs__elbv2.cfn-alb-listener-rule-routes-to-ecs-service-positive cloudformation__ecs__elbv2.cfn-alb-target-group-port-mismatch-negative cloudformation__ecs__elbv2.cfn-ecs-service-without-load-balancer-config-negative cloudformation__ecs__elbv2.cfn-security-group-blocks-alb-to-ecs-traffic-negative cloudformation__ecs__elbv2.cli-cloudformation-create-stack-then-ecs-service-list-tasks
Issue-Key: cloudformation__ecs__elbv2.cfn-alb-listener-rule-routes-to-ecs-service-positive|_http_get_retry#12|Timeout|status, body = _http_get(ctx, url, timeout=10)
Issue-Key: cloudformation__ecs__elbv2.cfn-alb-target-group-port-mismatch-negative|_http_get_retry#17|Timeout|status, body = _http_get(ctx, url, timeout=10)
Issue-Key: cloudformation__ecs__elbv2.cfn-ecs-service-without-load-balancer-config-negative|_http_get_retry#16|Timeout|status, body = _http_get(ctx, url, timeout=10)
Issue-Key: cloudformation__ecs__elbv2.cfn-security-group-blocks-alb-to-ecs-traffic-negative|_http_get_retry#12|Timeout|status, body = _http_get(ctx, url, timeout=10)
Issue-Key: cloudformation__ecs__elbv2.cli-cloudformation-create-stack-then-ecs-service-list-tasks|_poll_until#30|Timeout|time.sleep(interval_s)
Issue-Commit: dbbfc43f271727daf6a36bf70721e9eb6030bb1c

# cloudformation__ecs__elbv2: five CloudFormation ECS+ALB scenarios never reach a verdict — every one is cut off at its 60 s budget

> _Note: this issue was identified against the code base at commit `dbbfc43f271727daf6a36bf70721e9eb6030bb1c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: CloudFormation stacks that combine an ECS service with ELBv2 listeners/target groups never converge inside the check budget, so five separate ALB↔ECS behaviours are never observable.
AWS:      recorded behavior only. Each of these five checks deploys a stack (ALB + listener + listener rule + target group + ECS cluster/task definition/service, and for one of them the aws-cli path), then asserts one routing or refusal behaviour: a listener rule forwards to the ECS service; a target group whose port does not match the container port is refused/unhealthy; an ECS service declared without load-balancer configuration is not registered; a security group that blocks ALB→task traffic prevents delivery; and `aws cloudformation create-stack` followed by `aws ecs list-tasks` lists the service's tasks. In AWS each of those settles well inside the budget because the stack reaches a terminal status and the ECS/ELBv2 reads answer immediately.
Emulator: all five checks end with `Error: Check timed out after 60.0s` — no assertion is ever reached. The emulator does not drive these stacks (or the ECS/ELBv2 reads behind them) to a settled state inside the budget, so none of the five behaviours can be exercised.
Why:      a client cannot deploy the single most common containerised web shape (ALB in front of an ECS service) and observe it working: the deploy never settles, so every downstream verification — routing, health, task listing, the negative controls that prove a misconfiguration is refused — is unreachable. A CI pipeline built on this shape hangs rather than failing with a diagnosis.
Fix:      the CloudFormation provisioner's convergence for the ECS+ELBv2 resource set (AWS::ECS::Cluster/TaskDefinition/Service, AWS::ElasticLoadBalancingV2::LoadBalancer/Listener/ListenerRule/TargetGroup) — the stack and the service must reach a terminal/steady state promptly instead of leaving the poller spinning; the per-resource events on the stuck stack identify which handler does not settle.
Note on existing issues: the five checks below are listed as unowned by the replay report. Open issues on neighbouring checks in this same service (for example 20261111T095650Z-cloudformation-ecs-elbv2-2-net-new-causes-an.md, 20261113T124512Z-cloudformation-ecs-elbv2-an-aws-ecs-service-in.md and 20261114T034444Z-cloudformation-ecs-elbv2-the-ec2-launch-type-createservice.md) describe the same neighbourhood but carry different replay keys and do not cover these five.
Traces:   the five check ids in trace_ids.

## Evidence
All five checks fail identically, with no assertion reached:
  #### [mismatch] cloudformation__ecs__elbv2.cfn-alb-listener-rule-routes-to-ecs-service-positive
  Error: Check timed out after 60.0s
  #### [mismatch] cloudformation__ecs__elbv2.cfn-alb-target-group-port-mismatch-negative
  Error: Check timed out after 60.0s
  #### [mismatch] cloudformation__ecs__elbv2.cfn-ecs-service-without-load-balancer-config-negative
  Error: Check timed out after 60.0s
  #### [mismatch] cloudformation__ecs__elbv2.cfn-security-group-blocks-alb-to-ecs-traffic-negative
  Error: Check timed out after 60.0s
  #### [mismatch] cloudformation__ecs__elbv2.cli-cloudformation-create-stack-then-ecs-service-list-tasks
  Error: Check timed out after 60.0s

Expected: each check reaches its assertions (a routed request, a refused/unhealthy target, an unregistered service, a blocked delivery, a non-empty ListTasks) within the budget. Actual: the run is cut off before any of them, so the emulator produced no verdict at all for these five behaviours.

## Steps to Reproduce
1. CreateStack with an ELBv2 load balancer, listener, listener rule and target group plus an ECS cluster, Fargate task definition and service wired to the target group.
2. Poll DescribeStacks for a terminal status and ecs DescribeServices/ListTasks and elbv2 DescribeTargetHealth for a steady state.
3. Observe that the stack/service never settles: each of the five checks above is cut off at 60 s with no assertion reached.
