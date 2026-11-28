Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__emr__iam.multi-master-nodes-spread-in-emr-placement-group ec2__emr__iam.cluster-in-nonexistent-subnet-rejected ec2__emr__iam.service-role-without-placement-group-policy-rejected ec2__emr__iam.unknown-service-role-cluster-rejected
Issue-Key: ec2__emr__iam.multi-master-nodes-spread-in-emr-placement-group|run#14|AssertionError|assert (len(roles["MASTER"]), len(roles["CORE"])) == (3, 1), (
Issue-Key: ec2__emr__iam.cluster-in-nonexistent-subnet-rejected|_poll#7|Timeout|return last
Issue-Key: ec2__emr__iam.service-role-without-placement-group-policy-rejected|_poll#8|Timeout|return last
Issue-Key: ec2__emr__iam.unknown-service-role-cluster-rejected|_poll#7|Timeout|return last
Issue-Commit: 96996ff548d771d992d512709b10ef647dc44007

# ec2__emr__iam: a cluster reaches WAITING but is given no Amazon EC2 capacity — ListInstances reports an empty instance list, so all four Graviton (m7g.xlarge) cluster checks reach no verdict

> _Note: this issue was identified against the code base at commit `96996ff548d771d992d512709b10ef647dc44007`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunJobFlow

## Description
THIS ISSUE BUNDLES 1 CAUSE. It must be closed before submitting.

Cause 1 of 1: a cluster RunJobFlow accepted advances its own lifecycle to WAITING while no instance is ever provisioned for it, so ListInstances answers 200 with an empty Instances list and the cluster has no capacity at all.
Failing check ids for this cause: ec2__emr__iam.multi-master-nodes-spread-in-emr-placement-group, ec2__emr__iam.cluster-in-nonexistent-subnet-rejected, ec2__emr__iam.service-role-without-placement-group-policy-rejected, ec2__emr__iam.unknown-service-role-cluster-rejected.

  AWS: a cluster that RunJobFlow accepted and that reaches a non-terminal state has the Amazon EC2 instances its InstanceGroups asked for. Smithy (com.amazonaws.emr): ClusterState = STARTING | BOOTSTRAPPING | RUNNING | WAITING | TERMINATING | TERMINATED | TERMINATED_WITH_ERRORS — WAITING is a running cluster idling with no steps, so its primary and core nodes exist; ListInstances "Provides information for all active Amazon EC2 instances ... Amazon EC2 instances in any of the following states are considered active: AWAITING_FULFILLMENT, PROVISIONING, BOOTSTRAPPING, RUNNING" and returns one Instance per node, each carrying Ec2InstanceId ("the unique identifier of the instance in Amazon EC2"), InstanceGroupId and Status.State (InstanceState = AWAITING_FULFILLMENT | PROVISIONING | BOOTSTRAPPING | RUNNING | TERMINATED). For a request whose InstanceGroups ask for 3 MASTER + 1 CORE nodes, a WAITING cluster therefore lists 3 RUNNING MASTER instances and 1 RUNNING CORE instance, and those EC2 instances describe in the requested subnet/AZ. Recorded behavior for the timing only (a multi-primary m7g cluster takes minutes to reach WAITING).

  Emulator: DescribeCluster reports the cluster in WAITING and Ec2InstanceAttributes resolves correctly (Ec2SubnetId, Ec2AvailabilityZone and IamInstanceProfile all match the request), but ListInstances on that same cluster returns an empty list for every filter: the multi-primary check reads `{'MASTER': [], 'CORE': []}` where it requires (3, 1). The three remaining checks in this heading each begin with a positive control that polls DescribeCluster/ListInstances until EMR has launched at least one EC2 instance (or the cluster ends); because no instance ever appears and the cluster never ends, those three never get past their control and are cut off by the harness with `Error: Check timed out after 60.0s`, reaching no verdict on the behavior they test.
  Hypothesis: the capacity launch asks the emulator's Amazon EC2 for the instances over the in-process callback (services/service-com-amazonaws-emr-elasticmapreduce/src/state/cluster_instances.rs, launch_cluster_instances), and a launch Amazon EC2 refuses deliberately contributes NO instances while the cluster is still advanced; the launch names the hard-coded image `resolve:ssm:/aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-x86_64` for every cluster, while these four checks all request INSTANCE_TYPE m7g.xlarge (arm64/Graviton), so an architecture mismatch refusal from RunInstances would silently yield a cluster with zero nodes. Real Amazon EMR selects an EMR AMI matching the instance type's architecture.

  Why: a client that creates an EMR cluster on a Graviton instance type gets a cluster that reports itself healthy (WAITING) forever and never has a node — ListInstances is empty, nothing can be SSH'd or scheduled, and any automation that waits for capacity, counts nodes, or inspects per-node placement blocks indefinitely instead of either getting nodes or seeing the cluster fail. It also masks every downstream behavior that depends on nodes existing (placement-group membership, per-role subnet/AZ placement, node teardown).

  Fix: the cluster capacity launch in the emr service's state (services/service-com-amazonaws-emr-elasticmapreduce/src/state/cluster_instances.rs — launch_cluster_instances / LaunchPlacement) plus whatever drives the cluster state machine after RunJobFlow: the image the launch names must match the architecture of the requested instance type, and a launch the instance service refuses must not leave the cluster advancing to WAITING with no capacity — either the capacity is provisioned and reported by ListInstances, or the cluster must be settled into its failure state rather than presented as a healthy, empty cluster.

  Traces: ec2__emr__iam.multi-master-nodes-spread-in-emr-placement-group (direct assertion), ec2__emr__iam.cluster-in-nonexistent-subnet-rejected, ec2__emr__iam.service-role-without-placement-group-policy-rejected, ec2__emr__iam.unknown-service-role-cluster-rejected (each cut off in its launched-capacity positive control).

Relation to already-open issues (neither was reported as owning these checks in this replay): 20261114T075942Z-ec2-emr-iam-a-cluster-built-on-an.md attributes the three timeouts to the absence of an asynchronous TERMINATED_WITH_ERRORS / VALIDATION_ERROR settlement; the evidence here shows those three checks never reach the broken-reference leg at all — they stop earlier, in the positive control that requires launched capacity — so the missing capacity is the cause that gates them. 20261104T102001Z-emr-listinstances-stub.md (ListInstances scaffold stub) no longer applies: list_instances.rs is implemented and answers 200 from cluster.ec2_instances; the list is empty because nothing populates it.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from this checkout. Service heading `### ec2__emr__iam`: 1/5 passed, 4 failed.

[mismatch] ec2__emr__iam.multi-master-nodes-spread-in-emr-placement-group
  File "check_ec2__emr__iam_multi_master_nodes_spread_in_emr_placement_group.py", line 48, in run
    assert (len(roles["MASTER"]), len(roles["CORE"])) == (3, 1), (
  AssertionError: expected 3 RUNNING master and 1 RUNNING core instances, got {'MASTER': [], 'CORE': []}

That check had already passed, on the same cluster:
  - DescribeCluster Status.State == 'WAITING'
  - Ec2InstanceAttributes.(Ec2SubnetId, Ec2AvailabilityZone, IamInstanceProfile) == (requested subnet, its AZ, the instance-profile name)
and then read ListInstances(ClusterId=..., InstanceGroupTypes=['MASTER']) and (['CORE']), both of which returned no instances.

[mismatch] ec2__emr__iam.cluster-in-nonexistent-subnet-rejected
  Error: Check timed out after 60.0s
[mismatch] ec2__emr__iam.service-role-without-placement-group-policy-rejected
  Error: Check timed out after 60.0s
[mismatch] ec2__emr__iam.unknown-service-role-cluster-rejected
  Error: Check timed out after 60.0s

All three of those run, as their FIRST act after minting the roles, a positive control:
  control_id = _launch(ctx, _params(ctx, service_role, profile, subnet_id))
  got = _launched_or_failed(ctx, control_id)
  assert got[0] == "launched", f"control cluster {control_id} never launched capacity: {got!r}"
where _launched_or_failed polls DescribeCluster for a terminal state and otherwise
  _launched_ec2_ids(ctx, cluster_id) == sorted(i["Ec2InstanceId"] for i in emr.list_instances(ClusterId=...)["Instances"] if i.get("Ec2InstanceId"))
returning ("launched", ids) only once that list is non-empty. With ListInstances永 empty and the cluster never terminal, the poll never satisfies and the harness kills the check.

All four checks use the same parameters helper: ReleaseLabel 'emr-6.15.0', INSTANCE_TYPE 'm7g.xlarge', a private subnet of a VPC the check creates, EmrManaged master/core/service security groups, JobFlowRole = an instance profile with AmazonElasticMapReduceforEC2Role, ServiceRole = a role with AmazonElasticMapReduceRole (+ AmazonElasticMapReducePlacementGroupPolicy), and PlacementGroupConfigs=[{InstanceRole: MASTER, PlacementStrategy: SPREAD}].

Expected: a WAITING cluster lists its nodes (3 MASTER + 1 CORE RUNNING for the multi-primary request), each with an Ec2InstanceId that DescribeInstances resolves.
Actual: ListInstances returns Instances = [] for a WAITING cluster, and no EC2 instance exists for it.

## Steps to Reproduce
1. git clone the repo and let the behavioral suite build and boot the emulator from it.
2. Create a VPC and a private subnet in an AZ that offers m7g.xlarge; create the EMR service role (AmazonElasticMapReduceRole + AmazonElasticMapReducePlacementGroupPolicy) and an EC2 instance profile (AmazonElasticMapReduceforEC2Role).
3. RunJobFlow with ReleaseLabel 'emr-6.15.0', Instances.InstanceGroups = [{MASTER, m7g.xlarge, count 3}, {CORE, m7g.xlarge, count 1}], Instances.Ec2SubnetId = the subnet, KeepJobFlowAliveWhenNoSteps = true, PlacementGroupConfigs = [{InstanceRole: MASTER, PlacementStrategy: SPREAD}].
4. Poll DescribeCluster until Status.State == 'WAITING' (it does reach WAITING).
5. Call ListInstances(ClusterId=<id>) and ListInstances(ClusterId=<id>, InstanceGroupTypes=['MASTER']): expect 4 and 3 Instance records with Ec2InstanceId set and Status.State RUNNING; observe an empty Instances list, and no EC2 instance tagged aws:elasticmapreduce:job-flow-id=<id> in DescribeInstances.
6. Replay ec2__emr__iam.multi-master-nodes-spread-in-emr-placement-group for the direct assertion, and ec2__emr__iam.cluster-in-nonexistent-subnet-rejected / .service-role-without-placement-group-policy-rejected / .unknown-service-role-cluster-rejected, each of which hangs in its launched-capacity positive control.
7. For the hypothesis: repeat step 3 with an x86_64 instance type (e.g. m5.xlarge) and compare whether capacity appears.
