Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2__elbv2.asgdx-health-check-blocked-by-ingressless-sg-replaces-instances-until-subnet-admitted
Issue-Key: autoscaling__ec2__elbv2.asgdx-health-check-blocked-by-ingressless-sg-replaces-instances-until-subnet-admitted|_poll#23|Timeout|last = probe()
Issue-Commit: 8465be9c60d4839857b29976b5cf2d08632066fc

> _Resolved: both causes below are implemented. Cause 1's rule — a registration reaches the
> verdict its own checks decide, whatever health-check schedule its group carries — lives in
> `services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/src/state/target_health.rs`
> (`health_check_interval` / `verdict_spacing`) and is asserted by
> `services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/tests/all/target_health_verdict_span.rs`
> and `core/router/tests/all/asg_nlb_target_health_verdicts_at_any_cadence.rs`; Cause 2 replays as
> passed. The `asghc-nlb-target-stays-unhealthy-on-the-cluster-sg-until-the-subnet-cidr-is-admitted`
> check still ends at the replay harness's 60 s cap for a reason outside that surface, so its key
> is re-filed under `issues/things-that-do-not-work/` rather than recorded here as working._

# autoscaling__ec2__elbv2: 2 net-new causes — security-group-gated internal-NLB target health never reaches a verdict, and an ELB-health-checked group never replaces its unhealthy instance (both checks die at the 60 s cap)

> _Note: this issue was identified against the code base at commit `8465be9c60d4839857b29976b5cf2d08632066fc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTargetHealth

## Description
Two net-new root causes, both grounded in behavioral checks that are killed at the suite's 60 s per-check cap having never observed the state AWS reaches. Neither check is marked OWNED or ALREADY FILED by the replay report. The nearest open issues are 20261111T030957Z-autoscaling-ec2-elbv2-4-net-new-causes-target.md (target health synthesized 'healthy' with no probe; EC2/ALB target lifecycle never converges) and 20261115T034057Z-ec2-elbv2-reopens-the-alb-still-announces-an.md (a revoked target security-group ingress never changes target health) — they do not carry these checks' replay keys, so this is filed separately and names them for context.

Cause 1 of 2 — an internal NLB's health verdict does not follow the target's security-group ingress, and no verdict is produced at all.
AWS: Smithy (elasticloadbalancingv2): DescribeTargetHealth returns TargetHealthDescriptions[].TargetHealth{State,Reason,Description}; TargetHealthStateEnum = initial|healthy|unhealthy|unhealthy.draining|unused|draining|unavailable, TargetHealthReasonEnum includes Target.FailedHealthChecks, documented as "The load balancer received an error while establishing a connection to the target or the target response was malformed", and the model states that when State is healthy no Reason (and no Description) is provided. So for an instance target whose security group admits NO ingress on the health-check port, the first non-'initial' verdict must be State='unhealthy' with Reason='Target.FailedHealthChecks'; after AuthorizeSecurityGroupIngress admits TCP on the health-check port from the load balancer subnet's CIDR, the same target must leave 'unhealthy' and read State='healthy' with Reason absent, and the Auto Scaling group member must read HealthStatus='Healthy'. The target must also be registered on, and echoed back on, the target group's traffic port.
Emulator: neither verdict is ever observed. Both checks poll DescribeTargetHealth (every 10 s, against declared deadlines of 600–900 s) and are cut off by the harness: "Error: Check timed out after 60.0s", with no graded step. The emulator's target-health state therefore never advances in response to the security-group ingress that gates the probe — the unhealthy verdict before the fix and the healthy verdict after AuthorizeSecurityGroupIngress are both absent inside a budget in which the rest of the suite's control-plane calls complete.
Why: a client that diagnoses connectivity with DescribeTargetHealth gets no signal at all: the documented 'unhealthy'/'Target.FailedHealthChecks' that tells an operator the security group is blocking the probe never appears, and the 'healthy' transition that confirms the fix never appears either. Every runbook, deployment gate and autoscaler that waits for a target to go healthy hangs forever against the emulator.
Fix: ELBv2 target-health evaluation — the health-check state machine behind DescribeTargetHealth (and the EC2 security-group evaluation it must consult for the load balancer's subnet CIDR on the health-check port), plus the initial→unhealthy→healthy transitions it publishes. It must reach a verdict promptly rather than never.
Traces: autoscaling__ec2__elbv2.asghc-nlb-target-stays-unhealthy-on-the-cluster-sg-until-the-subnet-cidr-is-admitted, autoscaling__ec2__elbv2.asgdx-health-check-blocked-by-ingressless-sg-replaces-instances-until-subnet-admitted.

Cause 2 of 2 — an HealthCheckType='ELB' group never replaces the instance its ELB health check condemns, at constant desired capacity.
AWS: recorded behavior only (the model does not describe the reconciliation loop). An Auto Scaling group with HealthCheckType='ELB', HealthCheckGracePeriod=0 and Min=Max=Desired=1, whose single member fails the attached target group's health checks, marks that member Unhealthy, terminates it — recording exactly ONE scaling activity whose Description starts with "Terminating" and names that instance id — and launches a DIFFERENT instance in its place while DesiredCapacity stays at 1 (the replacement is never a scale-in). Once the health-check port is admitted on the security group, the current member converges to LifecycleState='InService' / HealthStatus='Healthy' and is registered in the target group on the traffic port with State='healthy'.
Emulator: no part of that loop is observed. The check polls DescribeAutoScalingGroups / DescribeInstances / DescribeScalingActivities for the first instance leaving the group and a different instance taking its place, and is killed at the 60 s cap ("Error: Check timed out after 60.0s") before any replacement, any "Terminating" activity for the condemned instance, or any healthy member appears.
Why: a client cannot observe or rely on ELB-health-driven instance replacement — the central self-healing behavior of an ELB-health-checked group. A deployment that waits for the group to heal, or a test that asserts the group replaced a bad instance without changing DesiredCapacity, never completes.
Fix: the Auto Scaling reconciliation loop (ELB health-status propagation into group member HealthStatus, the unhealthy-member terminate/replace path, and the scaling-activity records it writes), fed by the target-health state machine of Cause 1.
Traces: autoscaling__ec2__elbv2.asgdx-health-check-blocked-by-ingressless-sg-replaces-instances-until-subnet-admitted.

## Evidence
Expected (AWS): (1) DescribeTargetHealth on the ingress-less target group -> {State: 'unhealthy', Reason: 'Target.FailedHealthChecks'} as the first non-'initial' verdict; after AuthorizeSecurityGroupIngress(tcp 4000 from the NLB subnet CIDR) -> {State: 'healthy'} with no Reason, the target echoed on the traffic port, and the group member InService/Healthy. (2) For the ELB-health-checked group: the first instance leaves the group and is shutting-down/terminated, a different instance id is a member, DesiredCapacity still 1, and exactly one scaling activity whose Description starts with 'Terminating' names the first instance.

Actual (emulator): both checks produce no verdict at all — the replay report shows only 'Error: Check timed out after 60.0s' for each. No 'unhealthy'/'Target.FailedHealthChecks' verdict, no healthy transition after the ingress is admitted, no replacement instance and no 'Terminating' scaling activity is ever observed within the budget. Model grounding: TargetHealthStateEnum = [initial, healthy, unhealthy, unhealthy.draining, unused, draining, unavailable]; TargetHealthReasonEnum includes Target.FailedHealthChecks; TargetHealth.Reason doc: 'If the target state is healthy, a reason code is not provided.'

## Steps to Reproduce
1. Create a VPC with one subnet, and a security group with NO ingress rules.
2. Create a launch template whose user data serves HTTP 200 on :4000/health-check, and an HTTP target group on the traffic port with HealthCheckPort=4000, HealthCheckPath=/health-check, interval 10-20 s, UnhealthyThresholdCount 2-10.
3. Create an internal network load balancer in that subnet with a listener forwarding to the target group; wait for State.Code='active'.
4. Create an Auto Scaling group (Min=Max=Desired=1) in that subnet with the launch template and TargetGroupARNs=[tg]; use HealthCheckType='EC2' to observe target health only, or HealthCheckType='ELB' with HealthCheckGracePeriod=0 to observe replacement.
5. Poll DescribeTargetHealth until the member's state is not 'initial' -> expect {'unhealthy','Target.FailedHealthChecks'}; observed: no verdict before the check's budget ends.
6. AuthorizeSecurityGroupIngress(GroupId=sg, tcp 4000 from the subnet CIDR), then poll DescribeTargetHealth -> expect {'healthy', no Reason} and the group member InService/Healthy; observed: never reached.
7. With HealthCheckType='ELB', poll DescribeAutoScalingGroups/DescribeInstances/DescribeScalingActivities -> expect the first instance terminated, a different member, DesiredCapacity unchanged at 1 and one 'Terminating ...' activity; observed: none.
