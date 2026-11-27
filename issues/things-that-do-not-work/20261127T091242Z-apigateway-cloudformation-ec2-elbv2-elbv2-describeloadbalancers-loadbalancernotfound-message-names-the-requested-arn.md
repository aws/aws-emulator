Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudformation__ec2__elbv2.deploy-apigateway-vpclink-round-trip-and-delete
Issue-Key: apigateway__cloudformation__ec2__elbv2|DescribeLoadBalancers|LoadBalancerNotFound
Issue-Commit: 0e9afbb76f97c880d990543e0ccf2d5bfecf65a1

# elbv2: DescribeLoadBalancers' LoadBalancerNotFound message names the requested ARN instead of AWS's fixed "One or more load balancers not found"

> _Note: this issue was identified against the code base at commit `0e9afbb76f97c880d990543e0ccf2d5bfecf65a1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeLoadBalancers

## Description
Cause 1 of 1: after the NLB behind an API Gateway VPC link is deleted with its stack, DescribeLoadBalancers refuses the lookup with the right code but a message built from the request.

AWS: Smithy (com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10): DescribeLoadBalancers declares error LoadBalancerNotFoundException, errorKind client, httpError 400, awsQueryError {code: LoadBalancerNotFound, httpResponseCode: 400}. The model does not carry the message, so it is recorded behavior only: a DescribeLoadBalancers lookup that resolves no load balancer answers HTTP 400, Code `LoadBalancerNotFound`, Message `One or more load balancers not found`. That message is entirely fixed text — no variable part — and it is the same for one or many identifiers and for LoadBalancerArns or Names.

Emulator: same status and code, but the message is a template filled from the request: `Load balancers '[<identifiers as sent>]' not found`. In the replay the bracketed value was the deleted NLB's ARN; that ARN (its region, account, name and id) is a per-run value and is not the requirement — the defect is that the constant sentence is replaced by a request echo.

Why: a client confirming that a VPC link's NLB is really gone grades ELBv2's refusal text. Against AWS it matches a constant; against the emulator it matches nothing, so the teardown verification fails even though the resource was in fact deleted.

Fix: the elbv2 DescribeLoadBalancers handler's not-found refusal path (the LoadBalancerNotFoundException construction / message builder for the load-balancer lookup) must emit the fixed sentence without interpolating the request. The same cause appears under other service headings of this suite (see the cloudformation__ec2__elbv2 and cloudformation__cloudfront__ec2__elbv2 issues filed alongside this one).

Traces: apigateway__cloudformation__ec2__elbv2.deploy-apigateway-vpclink-round-trip-and-delete.

## Evidence
apigateway__cloudformation__ec2__elbv2.deploy-apigateway-vpclink-round-trip-and-delete:
  expect_error(lambda: elbv2.describe_load_balancers(LoadBalancerArns=[out["NlbArn"]]), *refused)
  AssertionError: expected ('LoadBalancerNotFound', 'One or more load balancers not found'),
  got ('LoadBalancerNotFound', "Load balancers '[arn:aws:elasticloadbalancing:us-west-2:000000000000:loadbalancer/net/sim2real-behav-Nlb-EE616EFD5280/9eadc0ced4683dba]' not found")

The HTTP status (400) and the error code match AWS; only the message's fixed text diverges.

## Steps to Reproduce
1. Deploy a stack with an AWS::ElasticLoadBalancingV2::LoadBalancer of type network plus an API Gateway VPC link over it; note the NLB ARN.
2. Delete the stack and wait for DELETE_COMPLETE.
3. Call elbv2 DescribeLoadBalancers(LoadBalancerArns=[<nlb arn>]).
4. AWS answers 400 / LoadBalancerNotFound / "One or more load balancers not found"; the emulator answers 400 / LoadBalancerNotFound / "Load balancers '[<nlb arn>]' not found".
