Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__elbv2.cfn-elbv2-gateway-load-balancer-geneve-target-group-lifecycle cloudformation__elbv2.cfn-elbv2-geneve-target-group-wrong-port-rejected
Issue-Key: cloudformation__elbv2.cfn-elbv2-gateway-load-balancer-geneve-target-group-lifecycle|_assert_gwlb#16|AssertionError|assert lb.get("Scheme") is None, f"a gateway load balancer carries no Scheme, got {lb.get('Scheme')!r}"
Issue-Key: cloudformation__elbv2.cfn-elbv2-geneve-target-group-wrong-port-rejected|run#18|AssertionError|assert bad["StackStatus"] == "ROLLBACK_COMPLETE", (
Issue-Commit: 9d0149c39ab5c73582ec7b48950d23789498e457

# cloudformation__elbv2: 2 net-new causes — a Gateway Load Balancer is described with a Scheme, and a GENEVE target group on a port other than 6081 is accepted

> _Note: this issue was identified against the code base at commit `9d0149c39ab5c73582ec7b48950d23789498e457`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateLoadBalancer

## Description
Two net-new root causes in the CloudFormation-provisioned ELBv2 Gateway Load Balancer surface.

Cause 1 of 2 — a Gateway Load Balancer carries a Scheme it must not have.
AWS: Smithy — `LoadBalancer.Scheme` (LoadBalancerSchemeEnum: internet-facing|internal) and `CreateLoadBalancer.Type` (LoadBalancerTypeEnum includes 'gateway'). A gateway load balancer has no scheme: AWS neither accepts nor reports one for Type='gateway', so DescribeLoadBalancers omits the Scheme member entirely for a GWLB (recorded behavior: the check asserts `lb.get("Scheme") is None`).
Emulator: DescribeLoadBalancers returns `Scheme: 'internet-facing'` for a load balancer created by CloudFormation with Type='gateway'.
Why: a client that branches on Scheme to tell internet-facing from internal load balancers mis-classifies every GWLB as internet-facing, and a template or tool that round-trips a described GWLB back into CreateLoadBalancer now sends a Scheme that AWS rejects for Type='gateway'.
Fix: the DescribeLoadBalancers serializer/state for ELBv2 — Scheme must be absent for a gateway-type load balancer (and CreateLoadBalancer must not default one in).
Traces: cloudformation__elbv2.cfn-elbv2-gateway-load-balancer-geneve-target-group-lifecycle.

Cause 2 of 2 — a GENEVE target group is created on a port other than 6081.
AWS: Smithy — `CreateTargetGroup.Protocol` (ProtocolEnum includes GENEVE), `CreateTargetGroup.Port` (range 1..65535) and error `InvalidConfigurationRequestException` (awsQueryError InvalidConfigurationRequest, HTTP 400). GENEVE target groups route only port 6081; any other port is refused. Through CloudFormation the refusal is relayed in the resource-handler envelope and the stack rolls back: the recorded reason is code `InvalidRequest` with message `Port must be 6081 for GENEVE target groups. (Service: ElasticLoadBalancingV2, Status Code: 400, Request ID: <masked>) (SDK Attempt Count: <masked>)` wrapped as `Resource handler returned message: "<inner>" (RequestToken: <uuid>, HandlerErrorCode: InvalidRequest)`. Fixed text: "Port must be 6081 for GENEVE target groups." and the "(Service: ElasticLoadBalancingV2, Status Code: 400, Request ID: …) (SDK Attempt Count: …)" envelope; the request id, attempt count and request token are per-run values, the port in the request is the value from the request.
Emulator: the AWS::ElasticLoadBalancingV2::TargetGroup with Protocol=GENEVE and Port=6082 is accepted and the stack reaches 'CREATE_COMPLETE' instead of 'ROLLBACK_COMPLETE'; no refusal is produced at all.
Why: a client's negative control passes where AWS fails the deploy, so an invalid GENEVE appliance fleet is provisioned and only fails at traffic time; conversely a template that AWS would reject is believed deployable.
Fix: CreateTargetGroup validation in the ELBv2 handler (GENEVE protocol implies Port 6081), so the CloudFormation resource handler relays the modeled InvalidConfigurationRequest refusal and the stack rolls back.
Traces: cloudformation__elbv2.cfn-elbv2-geneve-target-group-wrong-port-rejected.

## Evidence
Cause 1 — expected: DescribeLoadBalancers on a Type='gateway' load balancer returns no Scheme member.
Actual (replay):
  AssertionError: a gateway load balancer carries no Scheme, got 'internet-facing'
  (check_cloudformation__elbv2_cfn_elbv2_gateway_load_balancer_geneve_target_group_lifecycle.py line 335, _assert_gwlb)

Cause 2 — expected: a GENEVE target group declared on port 6082 is refused (InvalidRequest / "Port must be 6081 for GENEVE target groups. (Service: ElasticLoadBalancingV2, Status Code: 400, Request ID: <masked>) (SDK Attempt Count: <masked>)") and the stack ends 'ROLLBACK_COMPLETE'; the sibling stack on port 6081 succeeds.
Actual (replay):
  AssertionError: a target group on port 6082 must be refused; stack ended 'CREATE_COMPLETE'
  (check_cloudformation__elbv2_cfn_elbv2_geneve_target_group_wrong_port_rejected.py line 176)

## Steps to Reproduce
Cause 1:
1. CreateStack with a VPC, two subnets and an AWS::ElasticLoadBalancingV2::LoadBalancer of Type 'gateway' (no Scheme property), plus a GENEVE target group on port 6081; wait for CREATE_COMPLETE.
2. Call DescribeLoadBalancers on the stack's load balancer ARN.
Observed: the LoadBalancer carries Scheme='internet-facing'. Expected: no Scheme member.

Cause 2:
1. CreateStack with a VPC and an AWS::ElasticLoadBalancingV2::TargetGroup with Protocol='GENEVE', Port=6081 — it succeeds (positive control).
2. CreateStack with the same template but Port=6082; poll DescribeStacks for a terminal status and read the failing resource's ResourceStatusReason.
Observed: CREATE_COMPLETE, target group created. Expected: ROLLBACK_COMPLETE with the relayed 'Port must be 6081 for GENEVE target groups.' refusal under HandlerErrorCode InvalidRequest.
