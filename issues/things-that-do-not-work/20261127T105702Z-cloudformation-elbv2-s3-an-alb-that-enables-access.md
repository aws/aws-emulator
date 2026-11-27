Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__elbv2__s3.deploy-alb-access-logs-tls-only-bucket-refuses-a-grant-to-a-non-elb-account
Issue-Key: cloudformation__elbv2__s3.deploy-alb-access-logs-tls-only-bucket-refuses-a-grant-to-a-non-elb-account|run#25|AssertionError|assert status == "ROLLBACK_COMPLETE", (
Issue-Commit: 96db52f5b4a4b596ab227bd8becc0930845733a7

# cloudformation__elbv2__s3: an ALB that enables access logs to a bucket granting only the account owner is created instead of refused, so the stack reaches CREATE_COMPLETE

> _Note: this issue was identified against the code base at commit `96db52f5b4a4b596ab227bd8becc0930845733a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateLoadBalancer

## Description
Cause 1 of 1: enabling ALB access logs against a bucket whose policy does not grant the ELB log-delivery principal is accepted.
AWS: documented access-logging requirement (enable-access-logging) — when a load balancer is created (or modified) with access_logs.s3.enabled=true, ELBv2 verifies it can write to the bucket by putting a test object under <prefix>/AWSLogs/<account>/ELBAccessLogTestFile. If the bucket policy grants only the account owner and not the regional ELB log-delivery principal, the call is refused with HTTP 400, error class InvalidConfigurationRequest, message `Access Denied for bucket: <bucket>. Please check S3bucket permission` (fixed text; <bucket> is the bucket from the request). Through CloudFormation the AWS::ElasticLoadBalancingV2::LoadBalancer resource therefore ends CREATE_FAILED with a resource-handler reason carrying that message, Service ElasticLoadBalancingV2, Status Code 400 and HandlerErrorCode InvalidRequest, and the stack ends ROLLBACK_COMPLETE with the load balancer absent.
Emulator: the stack whose bucket policy grants only the owner ends CREATE_COMPLETE with no failed resources at all — the access-log permission is never verified, no test object is attempted, and the load balancer stays in service with access logging nominally enabled.
Why: a client gets a load balancer that AWS would have refused, and its access logs silently never arrive; the negative control that proves a misgranted bucket is rejected passes wrongly, so a real misconfiguration ships.
Fix: the ELBv2 access-log attribute validation reached from CreateLoadBalancer/ModifyLoadBalancerAttributes (the bucket-policy/test-object verification), which the CloudFormation load-balancer handler surfaces as the resource failure.
Traces: cloudformation__elbv2__s3.deploy-alb-access-logs-tls-only-bucket-refuses-a-grant-to-a-non-elb-account. A related open issue covers the opposite end of the same scenario family (an ungranted-bucket stack ending ROLLBACK_FAILED) and does not carry this replay key: 20261117T120522Z-cloudformation-elbv2-s3-the-stack-that-enables-alb.md.

## Evidence
`AssertionError: the stack granting the owner's account ended 'CREATE_COMPLETE': []` — expected ROLLBACK_COMPLETE with exactly one Balancer CREATE_FAILED reason of the form `Resource handler returned message: "Access Denied for bucket: <bucket>. Please check S3bucket permission (Service: ElasticLoadBalancingV2, Status Code: 400, Request ID: ...)" (RequestToken: ..., HandlerErrorCode: InvalidRequest)`, and afterwards a DescribeLoadBalancers by name refused LoadBalancerNotFound. The twin stack that grants the ELB log-delivery principal did reach CREATE_COMPLETE, so the divergence is isolated to the ungranted principal.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite.
2. Deploy two otherwise identical stacks, each with an S3 bucket plus bucket policy and an AWS::ElasticLoadBalancingV2::LoadBalancer with access_logs.s3.enabled=true, access_logs.s3.bucket=<its bucket>, access_logs.s3.prefix=frontdoor. One bucket policy grants the regional ELB log-delivery principal; the other grants only the account owner.
3. Wait for both stacks to settle: the ELB-granted stack must reach CREATE_COMPLETE (and the bucket must hold frontdoor/AWSLogs/<account>/ELBAccessLogTestFile), while the owner-granted stack must reach ROLLBACK_COMPLETE with the Balancer CREATE_FAILED as above. Observe that the owner-granted stack reaches CREATE_COMPLETE instead.
