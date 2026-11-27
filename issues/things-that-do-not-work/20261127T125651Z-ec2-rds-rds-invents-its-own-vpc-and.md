Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__rds.cluster-snapshot-vpc-id-is-the-cluster-subnet-group-vpc ec2__rds.instance-without-security-groups-gets-the-vpc-default-group
Issue-Key: ec2__rds.cluster-snapshot-vpc-id-is-the-cluster-subnet-group-vpc|run#14|AssertionError|assert snap.get("VpcId") == vpc_id, (
Issue-Key: ec2__rds.instance-without-security-groups-gets-the-vpc-default-group|run#11|AssertionError|assert attached == [default[0]["GroupId"]], (
Issue-Commit: ae5d8a902292096d9ed4c36fc40ed704ee301e98

# ec2__rds: RDS invents its own VPC and security-group ids instead of resolving them from EC2 state - a cluster snapshot's VpcId and a DB instance's default security group name resources that do not exist

> _Note: this issue was identified against the code base at commit `ae5d8a902292096d9ed4c36fc40ed704ee301e98`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDBClusterSnapshots

## Description
Two net-new causes, both the same shape: RDS mints a synthetic EC2 id rather than reading the EC2 resource the request resolves to. Each must be closed before submitting.

Cause 1 of 2: a DB cluster snapshot's VpcId is a fabricated id, not the cluster's DB subnet group's VPC.
AWS: recorded behavior only for the derivation (DBClusterSnapshot.VpcId is a plain String in the model). A cluster snapshot's VpcId is the VPC of the cluster's DB subnet group - the same VPC id EC2's DescribeVpcs returns for the subnets in that group.
Emulator: the snapshot reports `VpcId = 'vpc-cd3f34b29191d275e'` while the cluster's subnets live in `vpc-000000000000000a9`. The reported value is not any VPC the account holds - its shape (17 random hex characters) is not even an id the EC2 side ever issues in this run.
Why: VpcId on a snapshot is how a client decides whether a snapshot can be restored into a given network and which security groups apply; a fabricated id matches no VPC, so every such decision and every cross-reference against EC2 fails, and DescribeVpcs on the reported id is a not-found.
Fix: the DB cluster snapshot projection - resolve VpcId from the cluster's DBSubnetGroup's VpcId in EC2 state rather than generating one.
Traces: ec2__rds.cluster-snapshot-vpc-id-is-the-cluster-subnet-group-vpc.

Cause 2 of 2: a DB instance created with no security group is given a fabricated group id instead of the VPC's default security group.
AWS: recorded behavior only for the derivation. CreateDBInstance with no VpcSecurityGroupIds attaches the default security group of the DB subnet group's VPC, so DescribeDBInstances' VpcSecurityGroups carries exactly the GroupId EC2's DescribeSecurityGroups reports for GroupName `default` in that VPC.
Emulator: the instance reports `['sg-d6ada7e0a9b9e3957']` while the VPC's default group is `sg-000000000000000af` - again an id no EC2 security group has.
Why: a client that authorizes ingress to its database edits the group the instance reports; editing a group that does not exist (or editing the real default and seeing no effect) means connectivity can never be configured, and security audits that join RDS instances to EC2 security groups find a dangling reference.
Fix: the CreateDBInstance handler's network defaulting - look up the VPC's default security group in EC2 state and attach that id.
Traces: ec2__rds.instance-without-security-groups-gets-the-vpc-default-group.

## Evidence
Cause 1 - expected the cluster snapshot's VpcId to equal the cluster subnet group's VPC; actual "cluster snapshot VpcId is 'vpc-cd3f34b29191d275e'; the cluster lives in 'vpc-000000000000000a9'".
Cause 2 - expected the instance's VpcSecurityGroups to be exactly the VPC default group; actual "instance security groups ['sg-d6ada7e0a9b9e3957']; the VPC default group is 'sg-000000000000000af'".

## Steps to Reproduce
Cause 1: create a VPC with two subnets, a DB subnet group over them, an Aurora cluster in that subnet group, then CreateDBClusterSnapshot and read DescribeDBClusterSnapshots[].VpcId; compare it with the subnets' VpcId from EC2.
Cause 2: with the same VPC and DB subnet group, CreateDBInstance passing no VpcSecurityGroupIds; read DescribeDBInstances[].VpcSecurityGroups[].VpcSecurityGroupId and compare with DescribeSecurityGroups(Filters=[{'Name':'vpc-id',...},{'Name':'group-name','Values':['default']}]).
