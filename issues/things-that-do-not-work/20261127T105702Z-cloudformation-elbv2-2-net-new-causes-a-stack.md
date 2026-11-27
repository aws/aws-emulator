Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__elbv2.cfn-elbv2-tags-update-in-place-reaches-load-balancer-and-target-group cloudformation__elbv2.deploy-elbv2-2-duplicate-rule-priority-rejected-rolls-back-stack
Issue-Key: cloudformation__elbv2.cfn-elbv2-tags-update-in-place-reaches-load-balancer-and-target-group|run#17|AssertionError|assert got == {first["Lb"]: "green", first["Tg"]: "green"}, (
Issue-Key: cloudformation__elbv2.deploy-elbv2-2-duplicate-rule-priority-rejected-rolls-back-stack|run#29|AssertionError|assert code == ("LoadBalancerNotFound", "One or more load balancers not found"), (
Issue-Commit: 96db52f5b4a4b596ab227bd8becc0930845733a7

# cloudformation__elbv2: 2 net-new causes — a stack update does not re-tag the ELBv2 load balancer/target group, and the post-rollback LoadBalancerNotFound message echoes the ARN

> _Note: this issue was identified against the code base at commit `96db52f5b4a4b596ab227bd8becc0930845733a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
Two net-new root causes seen through CloudFormation-provisioned ELBv2 resources.

Cause 1 of 2: an in-place UpdateStack that changes a resource's Tags property never reaches the live ELBv2 resources.
AWS: recorded behavior only (AWS::ElasticLoadBalancingV2::LoadBalancer / ::TargetGroup resource handlers) — when the only change in an update is the Tags property, CloudFormation performs an in-place update and the resource handlers apply it with ELBv2 AddTags/RemoveTags, so after UPDATE_COMPLETE DescribeTags on both the load balancer and the target group reports the new tag value.
Emulator: after the update to env=green, DescribeTags for both the load balancer and the target group still report the pre-update value (env=blue) — the tag change is recorded in the template only and never applied to the ELBv2 state.
Why: a client that tags through CloudFormation (cost allocation, access control conditioned on tags, tag-based discovery) reads stale tags from the service that owns the resource, and tag-driven automation targets the wrong set.
Fix: the CloudFormation in-place update path for AWS::ElasticLoadBalancingV2::LoadBalancer and ::TargetGroup — the resource handlers' Tags diff must call the ELBv2 tag mutation.
Traces: cloudformation__elbv2.cfn-elbv2-tags-update-in-place-reaches-load-balancer-and-target-group.

Cause 2 of 2: after a rollback deletes the stack's ALB, the ELBv2 LoadBalancerNotFound refusal interpolates the requested ARN.
AWS: Smithy (elasticloadbalancingv2) — DescribeLoadBalancers models LoadBalancerNotFoundException, awsQueryError LoadBalancerNotFound, httpError 400; a lookup by LoadBalancerArns that resolves nothing is refused with the FIXED message `One or more load balancers not found`. (AWS reserves the interpolated `Load balancers '[<name>]' not found` form for lookups by Names.)
Emulator: answers 400 LoadBalancerNotFound with `Load balancers '[<the requested ARN>]' not found`, so the code matches but the message does not. The stack's rollback itself behaved as expected up to that point.
Why: a client (or a resource handler, or a cleanup script) that confirms a rolled-back resource is gone by matching AWS's message does not recognise the refusal.
Fix: the ELBv2 not-found refusal construction used by DescribeLoadBalancers for ARN-form lookups.
Traces: cloudformation__elbv2.deploy-elbv2-2-duplicate-rule-priority-rejected-rolls-back-stack. Related open issues for the same wording defect under other headings (none carries this replay key): 20261127T091242Z-cloudformation-ec2-elbv2-elbv2-both-not-found-refusals-describeloadbalancers-describetargetgroups-echo.md, 20261127T091242Z-apigateway-cloudformation-ec2-elbv2-elbv2-describeloadbalancers-loadbalancernotfound-message-names-the-requested-arn.md.

## Evidence
Cause 1: `AssertionError: after the update to env=green the resources are tagged {'<lb-arn>': 'blue', '<tg-arn>': 'blue'}` — expected 'green' on both (the ARNs are per-run values).
Cause 2: `assert code == ('LoadBalancerNotFound', 'One or more load balancers not found')` failed with `got ('LoadBalancerNotFound', "Load balancers '[arn:aws:elasticloadbalancing:us-west-2:000000000000:loadbalancer/app/<name>/<id>]' not found")` at HTTP 400.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite.
2. Cause 1: deploy a stack with an AWS::ElasticLoadBalancingV2::LoadBalancer and ::TargetGroup both tagged env=blue; UpdateStack with the same template but env=green; after UPDATE_COMPLETE call elbv2 DescribeTags on both physical resources.
3. Cause 2: deploy a stack whose second AWS::ElasticLoadBalancingV2::ListenerRule collides on priority so the stack ends ROLLBACK_COMPLETE, then call DescribeLoadBalancers(LoadBalancerArns=[<the ALB physical id from the CREATE_COMPLETE event>]) and compare Error.Message with 'One or more load balancers not found'.
