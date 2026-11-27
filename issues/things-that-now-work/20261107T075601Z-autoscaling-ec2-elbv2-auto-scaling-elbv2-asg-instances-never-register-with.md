Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2__elbv2.asg-attaches-multiple-target-groups autoscaling__ec2__elbv2.asg-deregisters-terminated-instance-from-target-group autoscaling__ec2__elbv2.asg-elb-health-check-triggers-instance-replacement autoscaling__ec2__elbv2.asg-eventbridge-rule-invokes-target-on-scale-event autoscaling__ec2__elbv2.asg-instance-launch-eventbridge-notification-delivered autoscaling__ec2__elbv2.asg-lifecycle-hook-event-triggers-on-instance-launch autoscaling__ec2__elbv2.asg-registers-new-ec2-instance-with-target-group autoscaling__ec2__elbv2.asg-target-group-registration-serves-http-and-feeds-back-health autoscaling__ec2__elbv2.asg-waits-for-elb-healthy-before-marking-instance-inservice autoscaling__ec2__elbv2.asgdx-health-check-blocked-by-ingressless-sg-replaces-instances-until-subnet-admitted autoscaling__ec2__elbv2.asghc-asg-registers-its-instance-at-the-group-port-into-a-target-group-with-no-nlb autoscaling__ec2__elbv2.asghc-nlb-target-stays-unhealthy-on-the-cluster-sg-until-the-subnet-cidr-is-admitted autoscaling__ec2__elbv2.deploy-cfn-asg-launch-template-ec2-instances autoscaling__ec2__elbv2.deploy-cfn-asg-scaling-policy-with-target-group autoscaling__ec2__elbv2.deploy-cfn-asg-without-elb-permission-instances-not-registered autoscaling__ec2__elbv2.deploy-cfn-stack-asg-elb-ec2-integration autoscaling__ec2__elbv2.asg-target-group-attachment-denied-without-permission autoscaling__ec2__elbv2.asgdx-create-group-with-nonexistent-target-group-rejected autoscaling__ec2__elbv2.asgdx-security-groups-refused-on-an-nlb-created-without-one-rejected autoscaling__ec2__elbv2.asghc-elb-type-group-replaces-its-unhealthy-instance-while-an-ec2-type-group-keeps-its-own autoscaling__ec2__elbv2.deploy-cfn-stack-missing-target-group-permission-fails-registration
Issue-Key: autoscaling__ec2__elbv2.asg-attaches-multiple-target-groups|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Key: autoscaling__ec2__elbv2.asg-deregisters-terminated-instance-from-target-group|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: autoscaling__ec2__elbv2.asg-elb-health-check-triggers-instance-replacement|run#16|Timeout|time.sleep(2.0)
Issue-Key: autoscaling__ec2__elbv2.asg-eventbridge-rule-invokes-target-on-scale-event|_poll_until#23|Timeout|time.sleep(interval_s)
Issue-Key: autoscaling__ec2__elbv2.asg-instance-launch-eventbridge-notification-delivered|_poll_sqs_for_message#11|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=10)
Issue-Key: autoscaling__ec2__elbv2.asg-lifecycle-hook-event-triggers-on-instance-launch|_poll_sqs_for_message#12|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=10)
Issue-Key: autoscaling__ec2__elbv2.asg-registers-new-ec2-instance-with-target-group|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Key: autoscaling__ec2__elbv2.asg-target-group-registration-serves-http-and-feeds-back-health|_poll_until#22|Timeout|last = probe()
Issue-Key: autoscaling__ec2__elbv2.asg-waits-for-elb-healthy-before-marking-instance-inservice|_poll_until#19|Timeout|last = probe()
Issue-Key: autoscaling__ec2__elbv2.asgdx-health-check-blocked-by-ingressless-sg-replaces-instances-until-subnet-admitted|_poll#16|Timeout|last = probe()
Issue-Key: autoscaling__ec2__elbv2.asghc-asg-registers-its-instance-at-the-group-port-into-a-target-group-with-no-nlb|_poll#11|Timeout|time.sleep(interval_s)
Issue-Key: autoscaling__ec2__elbv2.asghc-nlb-target-stays-unhealthy-on-the-cluster-sg-until-the-subnet-cidr-is-admitted|_poll#16|Timeout|time.sleep(interval_s)
Issue-Key: autoscaling__ec2__elbv2.deploy-cfn-asg-launch-template-ec2-instances|_poll_until#8|Timeout|time.sleep(interval_s)
Issue-Key: autoscaling__ec2__elbv2.deploy-cfn-asg-scaling-policy-with-target-group|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Key: autoscaling__ec2__elbv2.deploy-cfn-asg-without-elb-permission-instances-not-registered|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Key: autoscaling__ec2__elbv2.deploy-cfn-stack-asg-elb-ec2-integration|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Key: autoscaling__ec2__elbv2.asg-target-group-attachment-denied-without-permission|run#11|AssertionError|assert code == "AccessDenied", f"expected AccessDenied without the IAM permission, got {code!r}"
Issue-Key: autoscaling__ec2__elbv2.asgdx-create-group-with-nonexistent-target-group-rejected|_error#12|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: autoscaling__ec2__elbv2.asgdx-security-groups-refused-on-an-nlb-created-without-one-rejected|run#12|AssertionError|assert got == REFUSAL, f"SetSecurityGroups on an NLB created without security groups answered {got!r}, expected {REFUSAL!r}"
Issue-Key: autoscaling__ec2__elbv2|DescribeScalingActivities|NotImplemented
Issue-Key: autoscaling__ec2__elbv2.deploy-cfn-stack-missing-target-group-permission-fails-registration|run#11|AssertionError|assert final_status == "ROLLBACK_COMPLETE", (
Issue-Commit: 620c90c23a321396910fd66a692ef2fcecf33f7f

# Auto Scaling + ELBv2: ASG instances never register with target groups (16 checks hang), plus 5 refusal/representation causes

> _Note: this issue was identified against the code base at commit `620c90c23a321396910fd66a692ef2fcecf33f7f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAutoScalingGroup

## Description
Six net-new root causes behind the surviving autoscaling__ec2__elbv2 failures.

Cause 1 of 6: an ASG never registers its instances into its attached target groups, and no ASG-side state (target health, lifecycle, scaling activity, EventBridge notification) ever converges, so every waiter spins until the harness 60s budget ends.
AWS: a group created with TargetGroupARNs (or attached afterwards) registers each launched instance into every attached target group; DescribeTargetHealth then moves the target initial -> healthy (or unhealthy when the health check cannot pass), DescribeAutoScalingGroups reports the instance LifecycleState InService once the ELB health check passes, scale-in deregisters it, and the group's launch/terminate emits the EC2 Instance-Launch/Terminate Successful events (recorded behavior; elbv2 DescribeTargetHealth.TargetHealth.State, autoscaling Instance.LifecycleState).
Emulator: the target never appears/never leaves its initial state and the group's instances never reach InService, so each of the 16 checks below ends "Check timed out after 60.0s" with nothing having converged.
Why: the whole ASG-behind-a-load-balancer surface is unusable — no client can wait on a target becoming healthy, on replacement of an unhealthy instance, on deregistration at scale-in, or on a CFN stack whose ASG registers with a target group.
Fix: the ASG instance-lifecycle/registration machinery (autoscaling state advancing instances, its elbv2 target-group registration and health feedback).
Traces: autoscaling__ec2__elbv2.asg-attaches-multiple-target-groups, .asg-deregisters-terminated-instance-from-target-group, .asg-elb-health-check-triggers-instance-replacement, .asg-eventbridge-rule-invokes-target-on-scale-event, .asg-instance-launch-eventbridge-notification-delivered, .asg-lifecycle-hook-event-triggers-on-instance-launch, .asg-registers-new-ec2-instance-with-target-group, .asg-target-group-registration-serves-http-and-feeds-back-health, .asg-waits-for-elb-healthy-before-marking-instance-inservice, .asgdx-health-check-blocked-by-ingressless-sg-replaces-instances-until-subnet-admitted, .asghc-asg-registers-its-instance-at-the-group-port-into-a-target-group-with-no-nlb, .asghc-nlb-target-stays-unhealthy-on-the-cluster-sg-until-the-subnet-cidr-is-admitted, .deploy-cfn-asg-launch-template-ec2-instances, .deploy-cfn-asg-scaling-policy-with-target-group, .deploy-cfn-asg-without-elb-permission-instances-not-registered, .deploy-cfn-stack-asg-elb-ec2-integration.

Cause 2 of 6: an IAM-denied AttachLoadBalancerTargetGroups answers NotImplemented instead of an authorization refusal.
AWS: a principal without elasticloadbalancing/autoscaling attach permission is refused AccessDenied (HTTP 403) naming the principal and the action; the operation itself is modelled and reachable.
Emulator: answers NotImplemented ("operation ... is scaffolded but not implemented in this emulator"), so the authorization branch cannot be reached at all. The scaffold gap itself is already filed against services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/src/ops/attach_load_balancer_target_groups.rs; this check is listed as a live failure and is recorded here because implementing the operation must also make the denial path answer AccessDenied.
Why: a least-privilege test reads "not implemented" instead of "denied" and cannot tell a permissions defect from a missing feature.
Fix: AttachLoadBalancerTargetGroups handler + its authorization check.
Traces: autoscaling__ec2__elbv2.asg-target-group-attachment-denied-without-permission.

Cause 3 of 6: CreateAutoScalingGroup accepts TargetGroupARNs naming a target group that does not exist.
AWS: the create is refused ValidationError and no group is created (recorded behavior; only ValidationError is modelled for this input).
Emulator: the call SUCCEEDS and the group is stored with a dangling target-group ARN.
Why: a typo'd ARN is silently accepted; the group then reports attachments that resolve to nothing.
Fix: CreateAutoScalingGroup validation of TargetGroupARNs against elbv2 state.
Traces: autoscaling__ec2__elbv2.asgdx-create-group-with-nonexistent-target-group-rejected.

Cause 4 of 6: elbv2 SetSecurityGroups refusal message on an NLB created without security groups is emulator-authored.
AWS: InvalidConfigurationRequest with the fixed sentence "You cannot set security groups on a Network Load Balancer which was created without any security groups."
Emulator: same code, message "Security groups cannot be set on a network load balancer that was created without one".
Why: the code alone does not identify this boundary; clients and runbooks match the sentence.
Fix: elbv2 SetSecurityGroups validation message.
Traces: autoscaling__ec2__elbv2.asgdx-security-groups-refused-on-an-nlb-created-without-one-rejected.

Cause 5 of 6: DescribeScalingActivities is a scaffold stub, so no scaling-activity history exists.
AWS: DescribeScalingActivities returns the group's Activity records (StatusCode, Cause, Description) and an empty list for a group that never launched (recorded behavior; autoscaling Activity shape).
Emulator: NotImplemented (501-class ClientError) for every call.
Why: every "why did this group not launch" diagnosis — the documented way to read a failed launch — is impossible.
Fix: services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/src/ops/describe_scaling_activities.rs and the activity records the launch path should write.
Traces: autoscaling__ec2__elbv2.asghc-elb-type-group-replaces-its-unhealthy-instance-while-an-ec2-type-group-keeps-its-own.

Cause 6 of 6: a CloudFormation stack whose ASG registration cannot succeed still reaches CREATE_COMPLETE.
AWS: when the stack's role lacks the target-group registration permission the ASG resource fails and the stack rolls back to ROLLBACK_COMPLETE.
Emulator: the stack reports CREATE_COMPLETE.
Why: a deployment that AWS refuses looks successful, hiding the missing permission from any CI gate that keys on stack status.
Fix: the CloudFormation resource provisioner for AWS::AutoScaling::AutoScalingGroup — propagate the resource failure into stack status/rollback.
Traces: autoscaling__ec2__elbv2.deploy-cfn-stack-missing-target-group-permission-fails-registration.

## Evidence
From the behavioral replay:
- 16 checks (listed in Cause 1) fail with "Error: Check timed out after 60.0s" — no target ever registers, no instance reaches InService.
- asg-target-group-attachment-denied-without-permission [mismatch]: AssertionError: expected AccessDenied without the IAM permission, got 'NotImplemented'.
- asgdx-create-group-with-nonexistent-target-group-rejected [mismatch]: AssertionError: the call succeeded; real AWS rejects it, so there is no error code.
- asgdx-security-groups-refused-on-an-nlb-created-without-one-rejected [mismatch]: answered ('InvalidConfigurationRequest', 'Security groups cannot be set on a network load balancer that was created without one'), expected ('InvalidConfigurationRequest', 'You cannot set security groups on a Network Load Balancer which was created without any security groups.').
- asghc-elb-type-group-replaces-its-unhealthy-instance-while-an-ec2-type-group-keeps-its-own [error]: botocore ClientError (NotImplemented) calling DescribeScalingActivities: operation DescribeScalingActivities is scaffolded but not implemented in this emulator.
- deploy-cfn-stack-missing-target-group-permission-fails-registration [mismatch]: AssertionError: expected ROLLBACK_COMPLETE, got CREATE_COMPLETE.

## Steps to Reproduce
Replay the behavioral suite and re-run every check in trace_ids. The convergence cluster: create a VPC/subnets, an elbv2 target group (+ALB/NLB), a launch template and an ASG with TargetGroupARNs, then wait for DescribeTargetHealth to leave 'initial' and for the group's instance LifecycleState to become InService — in the emulator neither happens within 60s.
