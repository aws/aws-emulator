Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ec2__elbv2__opensearch.e2e-diagnose-alb-opensearch-bad-gateway-lifecycle cloudwatch__ec2__elbv2__opensearch.e2e-diagnose-alb-opensearch-stale-targets-with-alarm-config
Issue-Key: cloudwatch__ec2__elbv2__opensearch.e2e-diagnose-alb-opensearch-bad-gateway-lifecycle|phase#9|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Key: cloudwatch__ec2__elbv2__opensearch.e2e-diagnose-alb-opensearch-stale-targets-with-alarm-config|phase#9|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Commit: 84939cc283904302bdfe02403fef158918b6c804

# cloudwatch__ec2__elbv2__opensearch: an active VPC OpenSearch domain creates no elastic network interfaces — DescribeNetworkInterfaces filtered on description 'ES <domain>' and status 'in-use' returns an empty list

> _Note: this issue was identified against the code base at commit `84939cc283904302bdfe02403fef158918b6c804`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeNetworkInterfaces

## Description
Cause 1 of 1: an OpenSearch domain created with VPCOptions places no elastic network interfaces into its subnets, so nothing in EC2 represents the domain's in-VPC addresses. Both ALB/OpenSearch diagnosis lifecycles now get past their deploy phase and fail in the setup phase on exactly this.

AWS: creating a domain with VPCOptions makes OpenSearch Service place one elastic network interface per data node into the configured subnets, each with the fixed description text `ES ` followed by the domain name (the variable part is the request's DomainName) and a PrivateIpAddress drawn from the domain's own subnet CIDR. While the domain is active those ENIs have status `in-use`, so ec2 DescribeNetworkInterfaces with Filters [{Name: 'description', Values: ['ES <DomainName>']}, {Name: 'status', Values: ['in-use']}] returns at least one NetworkInterface. These are the addresses a client registers as ALB ip targets, and they are also what makes a pre-blue/green address 'stale'. This is recorded behaviour plus documented AWS behaviour (VPC support for OpenSearch Service domains); the Smithy model says nothing about ENI creation as a side effect of CreateDomain.

Emulator: that DescribeNetworkInterfaces call returns `NetworkInterfaces: []` for a domain that CreateDomain accepted and that DescribeDomain reports as active. No ENI with description `ES <DomainName>` exists in any status, so the domain has no in-VPC address at all.

Why: a client cannot discover the private addresses of its own VPC domain. Every workflow built on them breaks: registering a domain's nodes as ALB/NLB ip targets, distinguishing live node addresses from stale pre-blue/green ones, asserting the addresses fall inside the domain's subnet CIDR, and the stack-delete path that waits for the domain's ENIs to be released. Because no ENI is ever created, nothing refuses either — the domain simply looks like it has no network presence.

Fix: the OpenSearch CreateDomain handler's VPC side effects — it must create, and record in EC2's network-interface state, one in-use ENI per data node in the domain's VPCOptions subnets with description `ES <DomainName>` and a private IP from the subnet's CIDR, release them on DeleteDomain, and replace them on a blue/green change; EC2's DescribeNetworkInterfaces description/status filtering must then see them.

Note: the previously filed cloudwatch__ec2__elbv2__opensearch issues 20261111T083559Z-... and 20261111T112528Z-... describe these two checks failing in phase 1 (deploy) on a Listener stuck in CREATE_IN_PROGRESS; the checks now progress to phase 2 (setup) and fail on the missing ENIs, which those issues do not cover. 20261111T103332Z-cloudformation-iam-opensearch-... covers a different VPCOptions defect (dropped SecurityGroupIds), not the absent ENIs.

Traces: the two failing checks below, both failing with the identical setup-phase assertion.

## Evidence
Expected (AWS / recorded): ec2 DescribeNetworkInterfaces(Filters=[{Name:'description',Values:['ES <DomainName>']},{Name:'status',Values:['in-use']}]) returns >= 1 NetworkInterface for an active VPC domain, each PrivateIpAddress inside the domain's subnet CIDR and none of them equal to a stale pre-blue/green address.

Actual (emulator): the call returns an empty list, and the check fails with
  AssertionError: [phase 2/5 setup] the active domain should hold at least one in-use ENI, found []
(the fixed text of the assertion; the `[]` is the emulator's empty NetworkInterfaces list). The fixed part of the expected ENI description is `ES `, its variable part the DomainName from the CreateDomain request; the expected private addresses are variable values drawn from the domain's subnet.

Failing checks:
- cloudwatch__ec2__elbv2__opensearch.e2e-diagnose-alb-opensearch-bad-gateway-lifecycle
- cloudwatch__ec2__elbv2__opensearch.e2e-diagnose-alb-opensearch-stale-targets-with-alarm-config

## Steps to Reproduce
1. Deploy the alb-opensearch-stale-targets stack: a VPC with private subnets, an internal ALB with two target groups (dashboards, api), and an AWS::OpenSearchService::Domain with VPCOptions naming one of those subnets and a security group.
2. Wait for the domain to become active (DescribeDomain Processing=false).
3. Call ec2 DescribeNetworkInterfaces with Filters [{Name:'description', Values:['ES ' + DomainName]}, {Name:'status', Values:['in-use']}].
4. Observe NetworkInterfaces == [] instead of one in-use ENI per data node with a private IP from the domain's subnet; the setup phase of both diagnosis lifecycles then fails and no stale-vs-live target contrast can be set up.
