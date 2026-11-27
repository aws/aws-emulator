Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2__elbv2.cfn-elbv2-internal-load-balancer-in-place-update-retained-replacement-then-not-found cloudformation__ec2__elbv2.cfn-elbv2-listener-resource-lifecycle cloudformation__ec2__elbv2.cfn-elbv2-loadbalancer-resource-lifecycle cloudformation__ec2__elbv2.cfn-elbv2-target-group-in-place-update-retained-replacement-then-not-found cloudformation__ec2__elbv2.cfn-elbv2-targetgroup-resource-lifecycle
Issue-Key: cloudformation__ec2__elbv2.cfn-elbv2-internal-load-balancer-in-place-update-retained-replacement-then-not-found|run#42|AssertionError|assert got == _NOT_FOUND, f"DescribeLoadBalancers after the stack delete answered {got!r}"
Issue-Key: cloudformation__ec2__elbv2.cfn-elbv2-listener-resource-lifecycle|run#28|AssertionError|assert got == ("LoadBalancerNotFound", "One or more load balancers not found"), (
Issue-Key: cloudformation__ec2__elbv2.cfn-elbv2-loadbalancer-resource-lifecycle|run#31|AssertionError|assert (code, message) == ("LoadBalancerNotFound", "One or more load balancers not found"), (
Issue-Key: cloudformation__ec2__elbv2.cfn-elbv2-target-group-in-place-update-retained-replacement-then-not-found|run#44|AssertionError|assert got == _NOT_FOUND, f"DescribeTargetGroups after the stack delete answered {got!r}"
Issue-Key: cloudformation__ec2__elbv2.cfn-elbv2-targetgroup-resource-lifecycle|run#28|AssertionError|assert (code, message) == ("TargetGroupNotFound", "One or more target groups not found"), (
Issue-Commit: 0e9afbb76f97c880d990543e0ccf2d5bfecf65a1

# elbv2: both not-found refusals (DescribeLoadBalancers, DescribeTargetGroups) echo the requested ARNs instead of AWS's fixed "One or more ... not found" messages

> _Note: this issue was identified against the code base at commit `0e9afbb76f97c880d990543e0ccf2d5bfecf65a1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeLoadBalancers

## Description
Two net-new root causes, both on the elbv2 not-found refusal path reached after a CloudFormation stack delete removes the load balancer / target group.

Cause 1 of 2: DescribeLoadBalancers' LoadBalancerNotFound message interpolates the requested ARNs.
AWS: Smithy (com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10): DescribeLoadBalancers declares error LoadBalancerNotFoundException, errorKind client, httpError 400, awsQueryError {code: LoadBalancerNotFound, httpResponseCode: 400}. The model does not state the message, so it is recorded behavior only: for any DescribeLoadBalancers lookup that resolves no load balancer — by LoadBalancerArns or by Names, one identifier or several — the body carries Code `LoadBalancerNotFound` and the message `One or more load balancers not found`, which is wholly fixed text with no variable part and never names the identifier asked for.
Emulator: right code and status (LoadBalancerNotFound, 400), wrong message — a template that echoes the request: `Load balancers '[<identifiers as sent>]' not found`. The ARN inside the brackets (region, account, lb name, lb id) is a per-run value, not part of the requirement; the defect is that the fixed sentence is replaced by a request-derived one.
Failing checks: cloudformation__ec2__elbv2.cfn-elbv2-internal-load-balancer-in-place-update-retained-replacement-then-not-found, cloudformation__ec2__elbv2.cfn-elbv2-listener-resource-lifecycle, cloudformation__ec2__elbv2.cfn-elbv2-loadbalancer-resource-lifecycle.

Cause 2 of 2: DescribeTargetGroups' TargetGroupNotFound message interpolates the requested ARNs.
AWS: same model file: DescribeTargetGroups declares TargetGroupNotFoundException, errorKind client, httpError 400, awsQueryError {code: TargetGroupNotFound, httpResponseCode: 400}; recorded behavior for the message is the fixed sentence `One or more target groups not found`, again with no variable part.
Emulator: answers TargetGroupNotFound / 400 with `Target groups '[<identifiers as sent>]' not found`.
Failing checks: cloudformation__ec2__elbv2.cfn-elbv2-target-group-in-place-update-retained-replacement-then-not-found, cloudformation__ec2__elbv2.cfn-elbv2-targetgroup-resource-lifecycle.

Why: these are the refusals a client uses to confirm that a resource is really gone after a delete (here, after DeleteStack). A client grading "gone" on AWS's documented refusal string sees a message AWS never emits and cannot distinguish "deleted" from "something else went wrong"; worse, the emulator's message varies per request where AWS guarantees a constant, so logs and assertions built against AWS diverge on every call.

Fix: the elbv2 handlers' not-found refusal construction — the LoadBalancerNotFoundException path in the DescribeLoadBalancers handler and the TargetGroupNotFoundException path in the DescribeTargetGroups handler (the shared lookup/message builder for both identifier forms) — must emit the fixed sentences and stop interpolating the request.

Traces: the five failing checks named under the two causes above; the same two causes also surface under other service headings of this suite (see the apigateway__cloudformation__ec2__elbv2 and cloudformation__cloudfront__ec2__elbv2 issues filed alongside this one, and the ec2__elbv2 issue for the target-group message).

## Evidence
Cause 1 (DescribeLoadBalancers), after DeleteStack:
  cfn-elbv2-loadbalancer-resource-lifecycle:
    expected ("LoadBalancerNotFound", "One or more load balancers not found")
    got ('LoadBalancerNotFound', "Load balancers '[arn:aws:elasticloadbalancing:us-west-2:000000000000:loadbalancer/app/sim2real-behav-Alb-571654EBA84B/df47760ce54292ac]' not found")
  cfn-elbv2-listener-resource-lifecycle:
    got ('LoadBalancerNotFound', "Load balancers '[arn:aws:elasticloadbalancing:us-west-2:000000000000:loadbalancer/app/sim2real-behav-Alb-7FCD9C79C0AD/136fe6639b73e60d]' not found")
  cfn-elbv2-internal-load-balancer-in-place-update-retained-replacement-then-not-found:
    DescribeLoadBalancers after the stack delete answered ('LoadBalancerNotFound', "Load balancers '[arn:aws:elasticloadbalancing:us-west-2:000000000000:loadbalancer/app/sim2real-lb-b-629cf9c291d9/1b2e5d826b0c7eef]' not found")

Cause 2 (DescribeTargetGroups), after DeleteStack:
  cfn-elbv2-targetgroup-resource-lifecycle:
    expected ("TargetGroupNotFound", "One or more target groups not found")
    got ('TargetGroupNotFound', "Target groups '[arn:aws:elasticloadbalancing:us-west-2:000000000000:targetgroup/sim2real-TargetGro-A23F91D3CC09/89e0ea63605f835d]' not found")
  cfn-elbv2-target-group-in-place-update-retained-replacement-then-not-found:
    DescribeTargetGroups after the stack delete answered ('TargetGroupNotFound', "Target groups '[arn:aws:elasticloadbalancing:us-west-2:000000000000:targetgroup/sim2real-tg-b-a823a7635628/c71c44f91fa27349]' not found")

Status (400) and error codes match AWS in every case; only the message's fixed text diverges.

## Steps to Reproduce
1. Deploy a CloudFormation stack containing an AWS::ElasticLoadBalancingV2::LoadBalancer and an AWS::ElasticLoadBalancingV2::TargetGroup; note the ALB and target-group ARNs.
2. DeleteStack and wait for DELETE_COMPLETE.
3. Call elbv2 DescribeLoadBalancers(LoadBalancerArns=[<alb arn>]) — AWS: 400 LoadBalancerNotFound "One or more load balancers not found"; emulator: same code with "Load balancers '[<arn>]' not found".
4. Call elbv2 DescribeTargetGroups(TargetGroupArns=[<tg arn>]) — AWS: 400 TargetGroupNotFound "One or more target groups not found"; emulator: same code with "Target groups '[<arn>]' not found".
