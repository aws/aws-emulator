Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2__lambda.xsvc-elbv2-3-lambda-health-check-500-and-raise-mark-only-those-targets-unhealthy
Issue-Key: elbv2__lambda.xsvc-elbv2-3-lambda-health-check-500-and-raise-mark-only-those-targets-unhealthy|run#30|AssertionError|assert all(s != "healthy" for s, _ in seen[label]), (
Issue-Commit: b20300340ddfd95fa2f862f6c06bba5164a73eba

# elbv2__lambda: a newly registered Lambda target is reported 'healthy' before any health check has graded it, and its state then goes back to 'initial' before the real unhealthy verdict

> _Note: this issue was identified against the code base at commit `b20300340ddfd95fa2f862f6c06bba5164a73eba`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTargetHealth

## Description
Cause 1 of 1: DescribeTargetHealth on a Lambda-type target group serves an optimistic 'healthy' window for a target that has never passed a health check, and the state machine is non-monotonic (healthy -> initial -> unhealthy).

AWS: Smithy (elasticloadbalancingv2) TargetHealth.State / TargetHealth.Reason model the registration sequence: a target just added by RegisterTargets reports State 'initial' with Reason 'Elb.RegistrationInProgress' ("The target is in the process of being registered with the load balancer"), then State 'initial' with Reason 'Elb.InitialHealthChecking' ("The load balancer is still sending the target the minimum number of health checks required to determine its health status"). It leaves 'initial' only when the configured health check has actually graded it: to 'healthy' after HealthyThresholdCount consecutive passing checks (and then "If the target state is healthy, a reason code is not provided"), or to 'unhealthy' after UnhealthyThresholdCount consecutive failures with the matching reason — here 'Target.ResponseCodeMismatch' ("The health checks did not return an expected HTTP code") because the function's health path answers statusCode 500 while the group's Matcher is HttpCode '200'. A target whose health path never returns a matching code is therefore NEVER reported 'healthy' at any point in its life, and once it has left 'initial' it does not return to 'initial' while it stays registered and the group stays attached to a listener. There is no modeled or documented state in which a target that has only ever failed its health check reports 'healthy'.

Emulator: polling DescribeTargetHealth on the single-target Lambda group whose HealthCheckPath is /hc/fail (handler returns statusCode 500) observes, in order: ('initial','Elb.RegistrationInProgress'), ('healthy', no Reason) four consecutive polls, ('initial','Elb.InitialHealthChecking'), ('unhealthy','Target.ResponseCodeMismatch'). So the emulator (a) publishes 'healthy' with no Reason for a target that had not been health-checked at all and whose health path only ever answers 500, and (b) moves the target backwards from 'healthy' to 'initial'/'Elb.InitialHealthChecking' before producing the correct terminal verdict. The final unhealthy/Target.ResponseCodeMismatch verdict itself is now correct — only the states leading to it diverge.

Why: a client that gates on DescribeTargetHealth reads a passing fleet for a target that is broken. Deployment gates, readiness waiters and 'wait until healthy' loops (the overwhelmingly common pattern) sample the group once, see 'healthy', and proceed — exactly the opposite of the verdict AWS would give — and a monitor that treats the state machine as forward-only sees an impossible healthy->initial regression. Any logic that distinguishes 'not graded yet' (initial) from 'graded and passing' (healthy) is unusable, because the emulator uses 'healthy' for both.

Fix: the ELBv2 target-health state machine for Lambda-type target groups — the health-check evaluator and the state it serves through DescribeTargetHealth. A registered target must stay State 'initial' (Elb.RegistrationInProgress, then Elb.InitialHealthChecking) until the health check has produced HealthyThresholdCount passes or UnhealthyThresholdCount failures, must not be defaulted to 'healthy' before being graded, and must not transition back to 'initial' once graded.

Traces: elbv2__lambda.xsvc-elbv2-3-lambda-health-check-500-and-raise-mark-only-those-targets-unhealthy. Note: the open issue 20261110T143849Z-elbv2-lambda-a-lambda-target-whose-health-path.md names this same check, but for the earlier failure where the unhealthy verdict was never produced at all (its replay key is a _poll_until timeout); that verdict is now produced and the check fails later, on the transition sequence asserted here, so this cause is not carried by that issue. The open issue 20261109T153103Z-elbv2-lambda-2-net-new-causes-the-alb.md records every Lambda target being reported healthy regardless of its health path; it does not carry this check's replay key either.

## Evidence
Check elbv2__lambda.xsvc-elbv2-3-lambda-health-check-500-and-raise-mark-only-those-targets-unhealthy [mismatch]:
  AssertionError: the bad target was reported healthy at some point: [('initial', 'Elb.RegistrationInProgress'), ('healthy', None), ('healthy', None), ('healthy', None), ('healthy', None), ('initial', 'Elb.InitialHealthChecking'), ('unhealthy', 'Target.ResponseCodeMismatch')]
  (assertion: `all(s != "healthy" for s, _ in seen[label])` over the recorded (State, Reason) samples for the 'bad' and 'err' target groups)

Expected per AWS: for the group whose HealthCheckPath answers statusCode 500 against Matcher HttpCode '200', the observed (State, Reason) sequence contains only 'initial' states — Reason 'Elb.RegistrationInProgress' then 'Elb.InitialHealthChecking' — followed by ('unhealthy', 'Target.ResponseCodeMismatch'); the literal string 'healthy' never appears for that target, and no sample follows a 'healthy' or 'unhealthy' sample with an 'initial' one.

Actual: four consecutive samples report State 'healthy' with no Reason member, between the two 'initial' samples, before the terminal ('unhealthy', 'Target.ResponseCodeMismatch'). The reason strings themselves are the modeled enum values; the state ORDER is what diverges. (The counts of repeated samples above are poll artifacts of one run — the requirement is that 'healthy' never appears and that the state machine does not regress to 'initial'.)

## Steps to Reproduce
1. Create a VPC with public subnets, a permissive security group, and an internet-facing Application Load Balancer.
2. Create a Lambda function whose handler returns {"statusCode": 500, ...} for the path ending in /hc/fail and 200 for every other path; grant elasticloadbalancing.amazonaws.com lambda:InvokeFunction with SourceArn set to the target group.
3. elbv2 CreateTargetGroup with TargetType='lambda', HealthCheckEnabled=true, HealthCheckPath='/hc/fail', HealthCheckIntervalSeconds=5, HealthCheckTimeoutSeconds=4, HealthyThresholdCount=2, UnhealthyThresholdCount=2, Matcher={'HttpCode':'200'}; RegisterTargets with the function ARN.
4. Create an HTTP:80 listener and a rule forwarding to that group so health checks run.
5. Poll elbv2 DescribeTargetHealth on the group every 2 s from registration until the state settles, recording every (State, Reason) pair.
6. AWS: the recorded pairs are ('initial','Elb.RegistrationInProgress') / ('initial','Elb.InitialHealthChecking') then ('unhealthy','Target.ResponseCodeMismatch') — 'healthy' never appears. Emulator: ('healthy', no Reason) is reported for several polls, and the state afterwards regresses to ('initial','Elb.InitialHealthChecking') before the unhealthy verdict.
