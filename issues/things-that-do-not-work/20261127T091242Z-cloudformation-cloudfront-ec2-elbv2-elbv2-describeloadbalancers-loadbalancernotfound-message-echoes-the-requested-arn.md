Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__cloudfront__ec2__elbv2.deploy-cloudfront-vpc-origin-round-trip-and-delete
Issue-Key: cloudformation__cloudfront__ec2__elbv2|DescribeLoadBalancers|LoadBalancerNotFound
Issue-Commit: 0e9afbb76f97c880d990543e0ccf2d5bfecf65a1

# elbv2: DescribeLoadBalancers' LoadBalancerNotFound message echoes the requested ARN instead of AWS's fixed "One or more load balancers not found"

> _Note: this issue was identified against the code base at commit `0e9afbb76f97c880d990543e0ccf2d5bfecf65a1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeLoadBalancers

## Description
Cause 1 of 1: after the ALB behind a CloudFront VPC origin is deleted with its stack, DescribeLoadBalancers refuses the lookup with the correct code but a request-derived message.

AWS: Smithy (com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10): DescribeLoadBalancers declares error LoadBalancerNotFoundException, errorKind client, httpError 400, awsQueryError {code: LoadBalancerNotFound, httpResponseCode: 400}. The message is not in the model, so it is recorded behavior only: a lookup that resolves no load balancer answers HTTP 400, Code `LoadBalancerNotFound`, Message `One or more load balancers not found` — wholly fixed text, identical for one or many identifiers and for LoadBalancerArns or Names, and never naming the identifier asked for.

Emulator: same status and code, message templated from the request: `Load balancers '[<identifiers as sent>]' not found`. The ARN in the brackets is a per-run value (region, account, LB name and id differ every run) and is not part of the requirement; the defect is the replacement of AWS's constant sentence by a request echo.

Why: the client verifying that a CloudFront VPC origin's ALB was torn down grades ELBv2's refusal message. Against AWS it is a constant it can assert on; against the emulator it is a different string on every call, so the delete-verification step fails although the ALB is gone.

Fix: the elbv2 DescribeLoadBalancers handler's not-found refusal path (LoadBalancerNotFoundException construction / message builder) must emit the fixed sentence and stop interpolating the request. The same cause is filed under the other service headings it surfaces in (apigateway__cloudformation__ec2__elbv2, cloudformation__ec2__elbv2).

Traces: cloudformation__cloudfront__ec2__elbv2.deploy-cloudfront-vpc-origin-round-trip-and-delete.

## Evidence
cloudformation__cloudfront__ec2__elbv2.deploy-cloudfront-vpc-origin-round-trip-and-delete:
  expect_error(lambda: elbv2.describe_load_balancers(LoadBalancerArns=[alb_arn]), ...)
  AssertionError: expected ('LoadBalancerNotFound', 'One or more load balancers not found'),
  got ('LoadBalancerNotFound', "Load balancers '[arn:aws:elasticloadbalancing:us-west-2:000000000000:loadbalancer/app/sim2real-cf-vpc-Alb-F6F098612749/87554d59e8da0d81]' not found")

Status 400 and code LoadBalancerNotFound match AWS; the message's fixed text does not.

## Steps to Reproduce
1. Deploy a stack with an internal AWS::ElasticLoadBalancingV2::LoadBalancer fronted by a CloudFront VPC origin; note the ALB ARN.
2. Delete the stack and wait for DELETE_COMPLETE.
3. Call elbv2 DescribeLoadBalancers(LoadBalancerArns=[<alb arn>]).
4. AWS answers 400 / LoadBalancerNotFound / "One or more load balancers not found"; the emulator answers 400 / LoadBalancerNotFound / "Load balancers '[<alb arn>]' not found".
