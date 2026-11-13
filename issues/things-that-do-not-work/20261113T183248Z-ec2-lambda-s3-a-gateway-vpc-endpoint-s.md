Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__lambda__s3.s3-gateway-endpoint-config-round-trips
Issue-Key: ec2__lambda__s3.s3-gateway-endpoint-config-round-trips|_poll_until#19|Timeout|time.sleep(interval_s)
Issue-Commit: f2f8f36b8a9d5e45c8e23580863863f823670049

# ec2__lambda__s3: a Gateway VPC endpoint's PolicyDocument never converges through ModifyVpcEndpoint/ResetPolicy, so the round-trip check burns its whole budget

> _Note: this issue was identified against the code base at commit `f2f8f36b8a9d5e45c8e23580863863f823670049`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyVpcEndpoint

## Description
1 net-new root cause in the VPC-endpoint policy surface. Related open issues cover neighbouring CreateVpcEndpoint causes but carry not this replay key: 20261105T074335Z-ec2-lambda-s3-3-net-new-root-causes.md, 20261105T074141Z-ec2-lambda-s3-sqs-2-net-new-createvpcendpoint.md.

Cause 1 of 1: the policy document a Gateway endpoint reports never reaches the document the request installed.
AWS: an com.amazonaws.<region>.s3 Gateway endpoint reaches State 'available' and DescribeVpcEndpoints reports the PolicyDocument that CreateVpcEndpoint supplied, unchanged — it is not replaced by the service default; ModifyVpcEndpoint with PolicyDocument replaces it with exactly the document supplied; ModifyVpcEndpoint with ResetPolicy=true restores exactly the installed full-access default ({"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":"*","Action":"*","Resource":"*"}]}). Each of these is observable within seconds. Smithy: ModifyVpcEndpoint input PolicyDocument and ResetPolicy; VpcEndpoint members PolicyDocument and State. Reference: https://docs.aws.amazon.com/vpc/latest/privatelink/vpc-endpoints-access.html
Emulator: the check polls DescribeVpcEndpoints for each of those three conditions (state available, policy equal to the modified document, policy equal to the installed default) with a 120 s cap each and is killed by the 60 s replay budget — at least one of the three never converges, so no terminal value is observable at all (recorded behavior only; the replay reports only 'Check timed out after 60.0s').
Why: a client that installs or edits an endpoint policy and then waits for DescribeVpcEndpoints to show it waits forever and gets no error; the endpoint's effective access policy is unknowable, so nothing downstream can be gated on it.
Fix: the VpcEndpoint policy projection in the ec2 service — CreateVpcEndpoint must store the supplied PolicyDocument, ModifyVpcEndpoint must replace it, ResetPolicy must restore the installed default, and the endpoint must reach State 'available' so a poll terminates.
Traces: ec2__lambda__s3.s3-gateway-endpoint-config-round-trips.

## Evidence
Cause 1: Error: Check timed out after 60.0s. The check's three convergence polls (gateway endpoint available; policy equal to the supplied deny-all document after ModifyVpcEndpoint; policy equal to the installed full-access default after ResetPolicy=true) each allow 120 s, so a timeout means at least one condition was never observed.

## Steps to Reproduce
Cause 1: create a VPC and route table; CreateVpcEndpoint(VpcEndpointType=Gateway, ServiceName=com.amazonaws.<region>.s3, RouteTableIds=[...], PolicyDocument=<read-only doc>); DescribeVpcEndpoints and compare PolicyDocument with the supplied document; ModifyVpcEndpoint(PolicyDocument=<deny-all doc>) and re-read; ModifyVpcEndpoint(ResetPolicy=true) and re-read.
