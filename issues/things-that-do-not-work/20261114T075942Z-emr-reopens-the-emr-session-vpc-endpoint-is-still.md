Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr.concurrent-start-session-same-cluster-distinct-ids emr.e2e-studio-session-full-flow emr.e2e-studio-session-start-execute-terminate-flow emr.get-session-endpoint-certificate-blob-preserved emr.get-session-endpoint-for-terminated-cluster-rejected emr.get-session-endpoint-returns-endpoint-for-started-session emr.get-session-endpoint-returns-reachable-endpoint emr.list-sessions-newest-first emr.list-sessions-ordering-newest-first emr.list-sessions-orders-by-creation-time emr.session-endpoint-becomes-ready-within-ttl emr.session-idle-timeout-marks-session-stopped emr.session-idle-ttl-expires-without-explicit-terminate emr.session-lifecycle-start-active-terminate emr.session-lifecycle-start-get-terminate emr.session-tags-empty-vs-absent-preserved emr.session-terminate-then-get-reflects-terminated-state emr.start-session-non-studio-cluster-rejected emr.start-session-on-non-running-cluster-rejected emr.start-session-then-list-sessions-reflects-active-session emr.start-session-transitions-active-then-terminate-then-get-fails emr.terminate-session-already-terminated-rejected emr.terminate-session-twice-rejected
Issue-Shape-Id: com.amazonaws.emr#ElasticMapReduce
Issue-Key: emr|CreateVpcEndpoint|InvalidParameterValue
Issue-Commit: 84939cc283904302bdfe02403fef158918b6c804

# REOPENS: the EMR session VPC endpoint is still refused — CreateVpcEndpoint answers InvalidParameterValue 'the service aws.api.<region>.emr-service-cell01 has no private DNS name', blocking all 23 EMR session checks

> _Note: this issue was identified against the code base at commit `84939cc283904302bdfe02403fef158918b6c804`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateVpcEndpoint

## Description
REOPENS 20261105T093917Z-emr-6-net-new-root-causes-the-emr.md: its fix left the call refused.

THIS ISSUE BUNDLES 1 CAUSE. It must be closed before submitting.

Cause 1 of 1: the prerequisite interface VPC endpoint for the regional EMR session endpoint service cannot be created, because the emulator's endpoint-service entry for that service carries no private DNS name, so every EMR session check is refused before it reaches EMR.
  AWS: CreateVpcEndpoint with VpcEndpointType='Interface', ServiceName='aws.api.<region>.emr-service-cell01', SubnetIds=[<subnet>], SecurityGroupIds=[<group>] and PrivateDnsEnabled=true succeeds (HTTP 200) and returns a VpcEndpoint that then reaches State 'available'; the regional EMR session endpoint service HAS a private DNS name, so private DNS may be enabled on it. EC2 uses the query protocol and models no per-case error for an endpoint service, so this is recorded behavior only: the recordings show this call succeeding as the prerequisite of every EMR Spark-Connect session check. AWS reserves the 400 InvalidParameterValue sentence "Private DNS can't be enabled because the service <ServiceName> has no private DNS name" — fixed text with the requested ServiceName as its only variable — for endpoint services whose catalog entry genuinely has no private DNS name.
  Emulator: the identical request is refused 400 InvalidParameterValue "Private DNS can't be enabled because the service aws.api.us-west-2.emr-service-cell01 has no private DNS name" (the service token here is the region of the run and the one variable in the message). The earlier fix made the service NAME resolvable — it no longer answers InvalidServiceName "The Vpc Endpoint Service '...' does not exist" — but the catalog entry it added carries no private DNS name, so the call is still refused at the same step and no EMR operation is reached at all: 23 checks error on this prerequisite.
  Why: a client that stands up the private estate EMR sessions require cannot create the endpoint, so StartSession, ListSessions, GetSessionEndpoint and TerminateSession stay unreachable — the whole EMR interactive-session surface is ungradeable even where EMR implements it, and a caller that enables private DNS (the default posture for an interface endpoint to an AWS-owned service) gets a refusal AWS never issues.
  Fix: EC2's CreateVpcEndpoint handler and the VPC-endpoint-service catalog it validates PrivateDnsEnabled against — the 'aws.api.<region>.emr-service-cell*' entries must carry a private DNS name (and resolve/announce it through DescribeVpcEndpointServices), while the InvalidParameterValue refusal stays for services that really have none.
  Note: open issue 20261104T102001Z-emr-8-net-new-root-causes-createvpcendpoint-refuses.md describes a DIFFERENT CreateVpcEndpoint refusal (a gateway endpoint naming RouteTableIds answered 501 NotImplemented) and does not cover this private-DNS refusal of an interface endpoint.
  Traces: all 23 check ids listed in trace_ids; each one errors on this single call.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from this checkout (emr: 119/220 passed).

All 23 checks fail identically, before any EMR call:
  botocore.exceptions.ClientError: An error occurred (InvalidParameterValue) when calling the CreateVpcEndpoint operation: Private DNS can't be enabled because the service aws.api.us-west-2.emr-service-cell01 has no private DNS name

The call the checks make (verbatim from the check source):
  ec2.create_vpc_endpoint(VpcId=vpc_id, VpcEndpointType="Interface", SubnetIds=[subnet_id], SecurityGroupIds=[group], ServiceName="aws.api.<region>.emr-service-cell01", PrivateDnsEnabled=True, TagSpecifications=[...])
and they then poll DescribeVpcEndpoints for State == 'available'.

Expected: HTTP 200 with VpcEndpoint.VpcEndpointId, endpoint reaching 'available'.
Actual: HTTP 400 InvalidParameterValue with the private-DNS sentence above; nothing is created.

The previously fixed issue 20261105T093917Z-emr-6-net-new-root-causes-the-emr.md carried exactly 17 of these check ids under its Cause 1 (then refused with InvalidServiceName); the refusal has changed code and message but the call is still refused, and 6 further session checks now fail the same way.

## Steps to Reproduce
1. git clone the repo and let the behavioral suite build and boot the emulator from it.
2. ec2 CreateVpc + CreateSubnet + CreateSecurityGroup in the emulator.
3. Call ec2 CreateVpcEndpoint with VpcEndpointType='Interface', ServiceName='aws.api.<region>.emr-service-cell01', the subnet and group above, and PrivateDnsEnabled=True — expect 200 with a VpcEndpoint that reaches State 'available'; observe 400 InvalidParameterValue "Private DNS can't be enabled because the service aws.api.<region>.emr-service-cell01 has no private DNS name".
4. Replay any check in trace_ids (e.g. emr.session-lifecycle-start-get-terminate) and see it error on that call before reaching EMR.
