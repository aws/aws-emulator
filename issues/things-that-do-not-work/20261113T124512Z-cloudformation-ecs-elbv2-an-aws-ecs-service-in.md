Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ecs__elbv2.cfn-security-group-blocks-alb-to-ecs-traffic-negative
Issue-Key: cloudformation__ecs__elbv2.cfn-security-group-blocks-alb-to-ecs-traffic-negative|probe#10|IndexError|return ecs.describe_services(cluster=cluster, services=[service])["services"][0]
Issue-Commit: 6ec43c73f71e6013077e783d53ce3ba5d179d34d

# cloudformation__ecs__elbv2: an AWS::ECS::Service in a CREATE_COMPLETE stack is unknown to ECS — DescribeServices returns an empty services list

> _Note: this issue was identified against the code base at commit `6ec43c73f71e6013077e783d53ce3ba5d179d34d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeServices

## Description
Cause 1 of 1: a stack that declares an AWS::ECS::Service (alongside an ALB, target group and security groups) reports the resource created, but ECS has no such service, so DescribeServices answers with an empty `services` list and any poll on the service's state cannot proceed.
AWS: Smithy (ecs) DescribeServices takes `cluster` + `services` and returns a `services` list with one entry per existing service plus a `failures` entry (reason 'MISSING') for every identifier it cannot resolve; a service created by a stack that reached CREATE_COMPLETE always resolves, so `services[0]` exists and carries runningCount/desiredCount/deployments. A service that genuinely does not exist is reported as a failure entry, not as a silently empty list.
Emulator: `IndexError: list index out of range` on `describe_services(cluster=<stack cluster>, services=[<stack service>])["services"][0]` — the response contains neither a service nor a MISSING failure for it, so the CloudFormation-provisioned service exists only inside the stack's own bookkeeping.
Why: CREATE_COMPLETE stops being a truthful signal: every post-deploy step a client takes against the stack's service (describe, scale, update, wait for steady state, delete by name) sees nothing, and because the emulator answers 200 with an empty list rather than a MISSING failure, a client cannot even distinguish 'not created' from 'wrong name'.
Fix: the CloudFormation resource provider for AWS::ECS::Service must create the service in the ECS crate's state (or fail the stack), and ECS DescribeServices must report unresolvable identifiers in `failures` with reason 'MISSING' rather than omitting them.
Traces: cloudformation__ecs__elbv2.cfn-security-group-blocks-alb-to-ecs-traffic-negative. Open issue 20261104T045949Z-cloudformation-ecs-elbv2-cloudformation-provisioned-ecs-elbv2-resources.md concerns the same heading but does not carry this check's replay key.

## Evidence
File "check_cloudformation__ecs__elbv2_cfn_security_group_blocks_alb_to_ecs_traffic_negative.py", line 206, in probe: return ecs.describe_services(cluster=cluster, services=[service])["services"][0] -> IndexError: list index out of range. Expected: services[0] present for a stack-created service (or a failures entry with reason 'MISSING' when it truly does not exist).

## Steps to Reproduce
CreateStack with AWS::ECS::Cluster + AWS::ECS::TaskDefinition + AWS::ECS::Service + AWS::ElasticLoadBalancingV2::LoadBalancer/TargetGroup/Listener and a security group that blocks ALB->task traffic; wait for the stack to report the service created; call ecs DescribeServices(cluster=<stack cluster>, services=[<stack service>]) and inspect `services` and `failures`.
