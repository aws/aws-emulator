Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena__codebuild__dynamodb__ec2__ecs__kinesis__rds__route53__s3__sqs__ssm.cdk-storage-estate-two-regions-serves-and-destroys
Issue-Key: athena__codebuild__dynamodb__ec2__ecs__kinesis__rds__route53__s3__sqs__ssm.cdk-storage-estate-two-regions-serves-and-destroys|_run_cli#13|AssertionError|assert result.returncode == 0, (
Issue-Commit: 45e339952ace655163e1a6b7e05141367a36929a

# CloudFormation rejects Fn::GetAtt DnsEntries on AWS::EC2::VPCEndpoint — "Requested attribute DnsEntries does not exist in schema" fails the whole stack deploy

> _Note: this issue was identified against the code base at commit `45e339952ace655163e1a6b7e05141367a36929a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
1 net-new cause in this suite's CloudFormation resource-attribute surface.

Cause 1 of 1: the emulator's CloudFormation resource schema for `AWS::EC2::VPCEndpoint` does not carry the read-only attribute `DnsEntries`, so any template that resolves `Fn::GetAtt <VpcEndpointLogicalId>.DnsEntries` is refused at resource-update time and the stack deploy fails.
Failing check id: athena__codebuild__dynamodb__ec2__ecs__kinesis__rds__route53__s3__sqs__ssm.cdk-storage-estate-two-regions-serves-and-destroys

AWS: `AWS::EC2::VPCEndpoint` exposes `DnsEntries` as one of its read-only (Fn::GetAtt-able) attributes, alongside `Id`, `CreationTimestamp` and `NetworkInterfaceIds`. `Fn::GetAtt <VPCE>.DnsEntries` resolves to a list of strings, one per DNS entry of an Interface endpoint, each formatted `<HostedZoneId>:<DnsName>`. The backing data is modelled in EC2: Smithy `com.amazonaws.ec2#VpcEndpoint$DnsEntries` -> `DnsEntrySet` of `com.amazonaws.ec2#DnsEntry` with members `DnsName` and `HostedZoneId`, documented "(Interface endpoint) The DNS entries for the endpoint." This is exactly what the AWS CDK's `IInterfaceVpcEndpoint.vpcEndpointDnsEntries` compiles to (a `Fn::GetAtt ... DnsEntries` wrapped in `Fn::Select`/`Fn::Split` to pull the DNS name out of the `zoneId:dnsName` pair), which is why a CDK app that points a `AWS::Route53::RecordSet` CNAME at an interface endpoint depends on it. On AWS the GetAtt resolves and the RecordSet is created; there is no error.

Emulator: resolving that GetAtt is refused. The stack deploy reports the resource failure
  `sim2real-ds-<id>-Dns/ServiceCNAME/Resource  (AWS::Route53::RecordSet ServiceCNAMED322DBFA)`
  `Requested attribute DnsEntries does not exist in schema for AWS::EC2::VPCEndpoint`
and the stack (`...-Dns`) ends `failed to deploy`, so `cdk deploy --all` exits 1 instead of 0. Message anatomy: the fixed text is `Requested attribute ` + `<attribute>` + ` does not exist in schema for ` + `<resource type>`; `DnsEntries` and `AWS::EC2::VPCEndpoint` are the request-derived variable parts (the attribute named in the template's Fn::GetAtt and the referenced resource's Type). The wording itself is the correct CloudFormation wording for an attribute that genuinely is not in a resource's schema — the defect is that it is emitted for an attribute AWS *does* have, i.e. the schema registration for `AWS::EC2::VPCEndpoint` is missing `DnsEntries`, not that the refusal text is wrong.

Why: a client gets a hard deploy failure for a template AWS accepts. This is not a degraded value but a refusal, so the whole stack rolls back and every downstream stack in the app is never deployed — in this scenario one missing attribute aborts a thirty-stack, two-region estate before any of it can be exercised. Any CDK app using `InterfaceVpcEndpoint` plus private DNS/alias records (an extremely common PrivateLink pattern) cannot be deployed at all, and there is no workaround from the client side because CDK, not the user, authors the GetAtt.

Fix: CloudFormation's resource-type schema/attribute registration for `AWS::EC2::VPCEndpoint` — the read-only-attribute table consulted by the Fn::GetAtt resolver — must declare `DnsEntries` (plus the sibling read-only attributes `Id`, `CreationTimestamp`, `NetworkInterfaceIds`), and the VPCEndpoint resource handler must populate `DnsEntries` from the created endpoint's EC2 state as a list of `<HostedZoneId>:<DnsName>` strings so the GetAtt resolves to a usable value instead of being refused or resolving to null.

Traces: athena__codebuild__dynamodb__ec2__ecs__kinesis__rds__route53__s3__sqs__ssm.cdk-storage-estate-two-regions-serves-and-destroys (the `Dns` stack's `ServiceCNAME` AWS::Route53::RecordSet).

Note: no open issue covers this. The nearest open items are about EC2's own VPC-endpoint data plane (20261114T091241Z-ec2-5-net-new-causes-describevpcendpoints-omits-routetableids.md, 20261124T114924Z-ec2-describevpcendpointservices-servicedetailset-rows-are-minted-locally-missing.md) or about GetAtt on other resource types (e.g. 20261117T172826Z-cloudformation-dynamodb-...); none names the `AWS::EC2::VPCEndpoint` attribute schema.

## Evidence
Check: athena__codebuild__dynamodb__ec2__ecs__kinesis__rds__route53__s3__sqs__ssm.cdk-storage-estate-two-regions-serves-and-destroys — verdict [mismatch].

Expected (AWS): `cdk deploy --all` exits 0; `Fn::GetAtt <InterfaceVpcEndpoint>.DnsEntries` resolves to a list of `<HostedZoneId>:<DnsName>` strings, from which the CDK-generated `Fn::Select(1, Fn::Split(':', Fn::Select(0, ...)))` extracts the endpoint DNS name, and the `AWS::Route53::RecordSet` CNAME in the `Dns` stack is created with that value (the check then asserts the private zone holds exactly one CNAME to the endpoint DNS name with TTL 300).

Actual (emulator): `cdk deploy --all` exits 1. Deploy output tail:
  ❌  sim2real-ds-<id>-Dns failed to deploy
  ‣ DeploymentError: Resource updates failed:
  sim2real-ds-<id>-Dns/ServiceCNAME/Resource  (AWS::Route53::RecordSet ServiceCNAMED322DBFA)
    Requested attribute DnsEntries does not exist in schema for AWS::EC2::VPCEndpoint
Runner assertion: `AssertionError: \`cdk deploy --all\` exited 1, expected 0`.

Model citation: Smithy com.amazonaws.ec2#VpcEndpoint$DnsEntries -> DnsEntrySet of com.amazonaws.ec2#DnsEntry {DnsName, HostedZoneId}, "(Interface endpoint) The DNS entries for the endpoint." CloudFormation surfaces this as the read-only attribute `DnsEntries` on AWS::EC2::VPCEndpoint. (The `<id>` in the stack name and the `ServiceCNAMED322DBFA` logical-id hash are per-run values, not requirements.)

## Steps to Reproduce
1. Deploy a CloudFormation template (or CDK app) containing an `AWS::EC2::VPCEndpoint` of VpcEndpointType `Interface` in a VPC, plus an `AWS::Route53::RecordSet` of Type `CNAME` in a private hosted zone whose ResourceRecords value is derived from that endpoint, e.g.
   `{"Fn::Select": [1, {"Fn::Split": [":", {"Fn::Select": [0, {"Fn::GetAtt": ["Vpce", "DnsEntries"]}]}]}]}`
   (this is exactly what CDK emits for `InterfaceVpcEndpoint.vpcEndpointDnsEntries`).
2. Run CreateStack / `cdk deploy` against the emulator.
3. Observe the stack fails with resource error `Requested attribute DnsEntries does not exist in schema for AWS::EC2::VPCEndpoint` on the RecordSet, instead of resolving the GetAtt and reaching CREATE_COMPLETE.
4. Full scenario: replay behavioral check `athena__codebuild__dynamodb__ec2__ecs__kinesis__rds__route53__s3__sqs__ssm.cdk-storage-estate-two-regions-serves-and-destroys`; `cdk deploy --all` exits 1 at the `-Dns` stack.
