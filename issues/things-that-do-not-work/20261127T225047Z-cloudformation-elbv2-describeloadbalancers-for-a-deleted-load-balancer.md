Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__elbv2.cfn-elbv2-gateway-load-balancer-geneve-target-group-lifecycle
Issue-Key: cloudformation__elbv2|DescribeLoadBalancers|LoadBalancerNotFound
Issue-Commit: 37bf0949a3a203e7d1b7ebeb80108d58d67e6afd

# cloudformation + elbv2: DescribeLoadBalancers for a deleted load balancer answers a generic LoadBalancerNotFound message instead of naming the ARN

> _Note: this issue was identified against the code base at commit `37bf0949a3a203e7d1b7ebeb80108d58d67e6afd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeLoadBalancers

## Description
One net-new cause.

Cause 1 of 1: the LoadBalancerNotFound refusal DescribeLoadBalancers returns for an ARN that no longer exists does not name the ARN that was asked for.

AWS: DescribeLoadBalancers called with LoadBalancerArns naming a deleted load balancer is refused with HTTP 400, error code `LoadBalancerNotFound`, and the message `Load balancers '[<arn>, ...]' not found`. Fixed text: `Load balancers '[` ... `]' not found`; the variable part is the list of ARNs taken verbatim from the request. (Recorded behaviour for the exact wording; the modelled error shape is LoadBalancerNotFoundException.)

Emulator: the code is right but the message is the generic `One or more load balancers not found`, which names nothing from the request.

Why: a caller that deletes a Gateway Load Balancer through a stack delete and then proves absence has to parse the refusal to tell WHICH of the ARNs it asked about is gone. The emulator's message carries no ARN, so a multi-ARN probe cannot be attributed, and any assertion or log that quotes AWS's wording diverges.

Fix: the DescribeLoadBalancers handler's not-found path — build the message from the request's LoadBalancerArns list.

Traces: cloudformation__elbv2.cfn-elbv2-gateway-load-balancer-geneve-target-group-lifecycle (the same check's earlier steps — Gateway Load Balancer + GENEVE target group create, read-back and delete — all pass; only this refusal diverges).

## Evidence
expected ('LoadBalancerNotFound', "Load balancers '[arn:aws:elasticloadbalancing:us-west-2:000000000000:loadbalancer/gwy/sim2real-gwlb-78f37b07072a/ef3ec53a6fdcc693]' not found")
actual   ('LoadBalancerNotFound', 'One or more load balancers not found')

## Steps to Reproduce
1. CreateStack with an AWS::ElasticLoadBalancingV2::LoadBalancer of Type `gateway` plus a GENEVE target group; wait for CREATE_COMPLETE and record the load balancer ARN.
2. DeleteStack and wait for DELETE_COMPLETE.
3. Call DescribeLoadBalancers(LoadBalancerArns=[<that arn>]) and read the Error Code and Message.
