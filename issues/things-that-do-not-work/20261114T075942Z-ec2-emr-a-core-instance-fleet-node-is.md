Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__emr.core-fleet-launches-requested-type-with-emr-role-tags
Issue-Key: ec2__emr.core-fleet-launches-requested-type-with-emr-role-tags|run#15|AssertionError|assert shape[:2] == (INSTANCE_TYPE, subnet_id) and core_sg in shape[2], (
Issue-Commit: 84939cc283904302bdfe02403fef158918b6c804

# ec2__emr: a CORE instance-fleet node is launched outside the EmrManagedSlaveSecurityGroup the request names — DescribeInstances shows one unrelated group instead

> _Note: this issue was identified against the code base at commit `84939cc283904302bdfe02403fef158918b6c804`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunJobFlow

## Description
THIS ISSUE BUNDLES 1 CAUSE. It must be closed before submitting.

Cause 1 of 1: the EC2 instance EMR launches for a CORE instance fleet is not a member of the EmrManagedSlaveSecurityGroup the RunJobFlow request named.
  AWS: Instances.EmrManagedSlaveSecurityGroup is "The identifier of the Amazon EC2 security group for the core and task nodes" (Smithy: JobFlowInstancesConfig.EmrManagedSlaveSecurityGroup, which must be given together with EmrManagedMasterSecurityGroup), so every core/task instance EMR launches is attached to exactly that group (plus any AdditionalSlaveSecurityGroups), and EC2 DescribeInstances on the node lists that group id in Instances[].SecurityGroups[].GroupId. The node is launched in the requested subnet with the requested instance type and carries the aws:elasticmapreduce:job-flow-id / instance-group-role tags.
  Emulator: the one CORE node is launched with the right type and subnet but reports a SINGLE security group that is NOT the requested one — the requested EmrManagedSlaveSecurityGroup id never appears in the instance's SecurityGroups, so the caller's managed group is ignored when the instance is created.
  Why: a client that pre-creates and polices the EMR-managed groups (ingress/egress rules, group-referenced rules from other tiers, security review) ends up with nodes attached to a group it did not configure: nothing keyed to the named group — reachability, scoped rules, group-based IAM/VPC conditions — holds, and no group-level assertion about the cluster's nodes can be made.
  Fix: RunJobFlow's instance-launch path in the emr service — the security groups it attaches when it asks EC2 for fleet/instance-group capacity (EmrManagedMasterSecurityGroup for the primary, EmrManagedSlaveSecurityGroup for core/task, plus the Additional* lists) — and the security groups stored on the instance record EC2's DescribeInstances projects.
  Note: open issues 20261102T094623Z-ec2-emr-emr-adds-no-ingress-rules-to.md and 20261029T105400Z-ec2-emr-emr-writes-no-ingress-rules-onto-the-emrmanaged.md describe a DIFFERENT cause (no ingress RULES are written onto the managed groups) and do not cover the node's group MEMBERSHIP; neither carries this check id.
  Traces: ec2__emr.core-fleet-launches-requested-type-with-emr-role-tags.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from this checkout (ec2__emr: 1/4 passed).

ec2__emr.core-fleet-launches-requested-type-with-emr-role-tags:
  AssertionError: the core instance should be ('m5.2xlarge', 'subnet-000000000000000a4') in 'sg-000000000000000a8', got ('m5.2xlarge', 'subnet-000000000000000a4', ['sg-00000000000000092'])

The check launches a 1-unit CORE instance fleet (m5.2xlarge) with Instances.EmrManagedSlaveSecurityGroup set to a group it created, polls ListInstances(InstanceFleetType='CORE') for the Ec2InstanceId, then reads ec2.describe_instances for that id and requires the requested slave group id to be among the instance's SecurityGroups.
Expected: the requested EmrManagedSlaveSecurityGroup id present in Instances[].SecurityGroups.
Actual: one unrelated group id and no occurrence of the requested one (the particular ids are per-run values; the requirement is that the group named in the request is attached).

## Steps to Reproduce
1. git clone the repo and let the behavioral suite build and boot the emulator from it.
2. Create a VPC, a subnet and two security groups; mint the EMR service role and EC2 instance profile.
3. RunJobFlow with Instances.InstanceFleets (a 1-unit CORE fleet of m5.2xlarge), Ec2SubnetIds=[subnet], EmrManagedMasterSecurityGroup=<sg-master>, EmrManagedSlaveSecurityGroup=<sg-slave>.
4. Poll emr ListInstances(ClusterId, InstanceFleetType='CORE') for the Ec2InstanceId, then call ec2 DescribeInstances on it — expect <sg-slave> among SecurityGroups, observe a single unrelated group id (replay ec2__emr.core-fleet-launches-requested-type-with-emr-role-tags).
