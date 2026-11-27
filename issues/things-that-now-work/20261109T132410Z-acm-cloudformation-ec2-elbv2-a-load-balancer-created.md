Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm__cloudformation__ec2__elbv2.cfn-elbv2-listenercertificate-resource-lifecycle acm__cloudformation__ec2__elbv2.cfn-listenercertificate-nonexistent-certificate-rejected
Issue-Key: elastic-load-balancing-v2|DescribeListeners|LoadBalancerNotFound
Issue-Commit: cda81fc610349487f102bcc6f8ad570fe265d90b

# acm__cloudformation__ec2__elbv2: a load balancer created by a CREATE_COMPLETE stack is unknown to ELBv2 — DescribeListeners on the stack's AlbRef answers LoadBalancerNotFound

> _Note: this issue was identified against the code base at commit `cda81fc610349487f102bcc6f8ad570fe265d90b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeListeners

## Description
One net-new root cause (Cause 1 of 1).

Related open filings that do NOT cover this replay: 20261024T235905Z-elbv2-describerules-answers-listenernotfound-for-the-listener-arn.md (DescribeRules/ListenerNotFound) and 20261104T045949Z-cloudformation-ecs-elbv2-cloudformation-provisioned-ecs-elbv2-resources.md (CFN-provisioned ECS/ELBv2 resources absent from their owning services) describe the same family, but neither is filed under this service heading nor carries these check ids, so both failures survive unowned.

Cause 1 of 1: an AWS::ElasticLoadBalancingV2::LoadBalancer in a stack that reaches CREATE_COMPLETE is not registered in ELBv2 state, so the ARN the stack's Ref/Outputs publish is not resolvable by ELBv2 read operations.
AWS: Smithy (elasticloadbalancingv2) DescribeListeners(LoadBalancerArn) -> Listeners, with modeled errors ListenerNotFoundException (awsQueryError ListenerNotFound, 400), LoadBalancerNotFoundException (awsQueryError LoadBalancerNotFound, 400) and UnsupportedProtocolException. LoadBalancerNotFound is the refusal for an ARN naming a load balancer that does not exist. Recorded AWS behavior: CloudFormation's Ref for AWS::ElasticLoadBalancingV2::LoadBalancer is the load balancer's ARN, and once the stack is CREATE_COMPLETE that exact ARN resolves in ELBv2 — DescribeListeners(LoadBalancerArn=<Ref>) returns the stack's AWS::ElasticLoadBalancingV2::Listener (Port 443, Protocol HTTPS, the SslPolicy the template set) and DescribeListenerCertificates returns the default certificate plus any AWS::ElasticLoadBalancingV2::ListenerCertificate the stack added. A CREATE_COMPLETE stack whose resource is absent from its owning service is not a state AWS can be in.
Emulator: the stack reaches CREATE_COMPLETE and publishes Outputs AlbRef/ListenerRef, but the very first ELBv2 read of AlbRef is refused: HTTP 400 LoadBalancerNotFound, message "Load balancer '<load-balancer-arn>' not found" (the ARN in the message is the value from the request; replay values 'arn:aws:elasticloadbalancing:us-west-2:000000000000:loadbalancer/app/sim2real-behav-Alb-9BDACD0765E1/a29bcd6cb6677780' and '...-Alb-1B296995725B/da4274ac8cbd603e' are per-run). Either the resource was never written to ELBv2 state or Ref publishes an ARN ELBv2 does not own; either way the deployed load balancer and its listener are invisible to the service that must serve them.
Why: a client that deploys an ALB by template and then drives it through the ELBv2 API — the standard IaC flow — gets a successful CREATE_COMPLETE followed by a not-found refusal for the resource the stack says it created. Nothing downstream can work: listener lookup, listener-certificate attach/detach via AWS::ElasticLoadBalancingV2::ListenerCertificate, the stack update that drops a certificate, and the post-delete not-found assertions are all unreachable, and the two failures are reported as errors rather than mismatches because the chain breaks at the first call.
Fix: the CloudFormation resource provider for AWS::ElasticLoadBalancingV2::LoadBalancer (and the Listener/ListenerCertificate providers that depend on it) — provisioning must create the load balancer in ELBv2's own state and publish that resource's real ARN as the physical id / Ref, so ELBv2 DescribeListeners, DescribeListenerCertificates and DescribeLoadBalancers resolve it.
Traces: acm__cloudformation__ec2__elbv2.cfn-elbv2-listenercertificate-resource-lifecycle; acm__cloudformation__ec2__elbv2.cfn-listenercertificate-nonexistent-certificate-rejected.

## Evidence
Replay of suite simparity-behavioral-aws-v1: acm__cloudformation__ec2__elbv2 0/2 (0%), 2 failed — neither is marked ALREADY FILED or OWNED.

[error] acm__cloudformation__ec2__elbv2.cfn-elbv2-listenercertificate-resource-lifecycle:
  File "client.py", line 1094, in _make_api_call -> raise error_class(parsed_response, operation_name)
  botocore.errorfactory.LoadBalancerNotFoundException: An error occurred (LoadBalancerNotFound) when calling the DescribeListeners operation: Load balancer 'arn:aws:elasticloadbalancing:us-west-2:000000000000:loadbalancer/app/sim2real-behav-Alb-9BDACD0765E1/a29bcd6cb6677780' not found

[error] acm__cloudformation__ec2__elbv2.cfn-listenercertificate-nonexistent-certificate-rejected:
  same frame and code, ARN 'arn:aws:elasticloadbalancing:us-west-2:000000000000:loadbalancer/app/sim2real-behav-Alb-1B296995725B/da4274ac8cbd603e'

Both checks deploy one template (VPC, two subnets, a security group, an internal AWS::ElasticLoadBalancingV2::LoadBalancer, an HTTPS:443 AWS::ElasticLoadBalancingV2::Listener with Certificates=[{Ref CertAArn}] and a fixed-response default action, optionally an AWS::ElasticLoadBalancingV2::ListenerCertificate), assert the stack status is CREATE_COMPLETE, then read Outputs {AlbRef: {Ref Alb}, ListenerRef: {Ref Listener}} and call elbv2.describe_listeners(LoadBalancerArn=AlbRef) through the paginator. Expected: the ALB's only listener, with Port 443, Protocol HTTPS, SslPolicy ELBSecurityPolicy-2016-08, and DescribeListenerCertificates reporting CertA default plus CertB attached. Actual: the paginated DescribeListeners call is refused HTTP 400 LoadBalancerNotFound for the ARN the stack itself published, so no listener or certificate assertion is ever evaluated. The stack/ALB names and ARNs are per-run values.

Smithy consulted: elasticloadbalancingv2 DescribeListeners (input LoadBalancerArn; errors ListenerNotFoundException/ListenerNotFound, LoadBalancerNotFoundException/LoadBalancerNotFound, UnsupportedProtocolException, all 400 client errors).

## Steps to Reproduce
1. Clone the repo and replay the behavioral suite; both check ids in trace_ids reproduce.
2. acm.import_certificate two self-signed leaves (CertA, CertB).
3. cloudformation.create_stack with a template containing AWS::EC2::VPC + two AWS::EC2::Subnet + AWS::EC2::SecurityGroup, an internal AWS::ElasticLoadBalancingV2::LoadBalancer ('Alb'), an AWS::ElasticLoadBalancingV2::Listener on Port 443/Protocol HTTPS with Certificates=[{CertificateArn: {Ref CertAArn}}] and a fixed-response default action, and Outputs AlbRef={Ref Alb}, ListenerRef={Ref Listener}; pass CertAArn=<CertA>.
4. Poll DescribeStacks until CREATE_COMPLETE (it is reached), then read Outputs.AlbRef.
5. elbv2.describe_listeners(LoadBalancerArn=<AlbRef>): AWS returns the stack's listener; the emulator answers HTTP 400 LoadBalancerNotFound "Load balancer '<arn>' not found".
6. Re-replay the two check ids in trace_ids to confirm the fix.
