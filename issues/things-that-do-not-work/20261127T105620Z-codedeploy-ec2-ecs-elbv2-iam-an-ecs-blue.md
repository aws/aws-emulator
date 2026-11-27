Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codedeploy__ec2__ecs__elbv2__iam.ecs-all-at-once-blue-green-pins-primary-task-set-to-new-revision
Issue-Key: codedeploy__ec2__ecs__elbv2__iam.ecs-all-at-once-blue-green-pins-primary-task-set-to-new-revision|_poll_until#17|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Commit: 96db52f5b4a4b596ab227bd8becc0930845733a7

# codedeploy__ec2__ecs__elbv2__iam: an ECS blue/green deployment's load balancer never serves the origin task set's response, so the all-at-once cutover cannot be observed

> _Note: this issue was identified against the code base at commit `96db52f5b4a4b596ab227bd8becc0930845733a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeployment

## Description
Cause 1 of 1: the ALB fronting an EXTERNAL/CODE_DEPLOY-controlled ECS service never returns the body served by the primary task set.
AWS: an ECS service with deploymentController CODE_DEPLOY and a primary task set registered behind an ALB target group serves that task set's response through the load balancer: a GET to the listener returns the origin revision's body before the deployment, and after an all-at-once CodeDeploy deployment the listener returns the replacement revision's body while the primary task set is pinned to the new task definition revision (recorded behavior; the data-plane response is not described by any model).
Emulator: the load balancer never serves the origin body at all — the check's bounded poll of the listener for the pre-deployment body expires before the deployment is even started, so the cutover is never reached. The failure is in the emulator's ALB-to-ECS data path (listener/target-group forwarding to the task set's task), not in CodeDeploy's API surface.
Why: a client cannot validate a blue/green cutover, which is observable only through the load balancer: the origin response, the switch to the replacement response and the pinning of the primary task set are all invisible. Any traffic-shifting workflow (CodeDeploy, task-set weighting, rollback on a failed deployment) is unverifiable.
Fix: the ALB listener/target-group forwarding path to ECS task-set targets (the ELBv2 data-plane in services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/src/ together with task-set target registration in the ECS service), so a registered healthy task set's response is served through the listener.
Traces: check codedeploy__ec2__ecs__elbv2__iam.ecs-all-at-once-blue-green-pins-primary-task-set-to-new-revision.

## Evidence
TimeoutError: ALB at '<listener host:port>' serving body 'origin-revision': the run budget ended first (raised from _await_alb_body/_poll_until). Expected: a GET through the ALB listener returns the origin task set's body before the deployment, and the replacement body after the all-at-once CodeDeploy deployment, with the primary task set pinned to the new task definition revision. The host and port shown are per-run values.

## Steps to Reproduce
Replay check codedeploy__ec2__ecs__elbv2__iam.ecs-all-at-once-blue-green-pins-primary-task-set-to-new-revision: create a VPC/subnets, an ALB with a listener and two target groups, an ECS cluster and a service with deploymentController CODE_DEPLOY, CreateTaskSet for the origin revision registered in the blue target group, then poll the listener for the origin body before calling codedeploy.CreateDeployment for the all-at-once cutover.
