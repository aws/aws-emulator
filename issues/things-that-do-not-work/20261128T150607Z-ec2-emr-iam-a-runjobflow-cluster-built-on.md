Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__emr__iam.cluster-in-nonexistent-subnet-rejected ec2__emr__iam.unknown-service-role-cluster-rejected
Issue-Key: ec2__emr__iam.cluster-in-nonexistent-subnet-rejected|_poll#6|Timeout|time.sleep(interval_s)
Issue-Key: ec2__emr__iam.unknown-service-role-cluster-rejected|_poll#6|Timeout|time.sleep(interval_s)
Issue-Commit: 70eb2a3dfdc5763d75751d1a0ee4a95a331f97e0

# ec2__emr__iam: a RunJobFlow cluster built on an unresolvable reference is never given EC2 capacity and never settles TERMINATED_WITH_ERRORS/VALIDATION_ERROR, so both broken-reference checks are cut off before any verdict (1 cause)

> _Note: this issue was identified against the code base at commit `70eb2a3dfdc5763d75751d1a0ee4a95a331f97e0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunJobFlow

## Description
THIS ISSUE BUNDLES 1 CAUSE. It must be closed before submitting.

Relation to already-open issues (neither was reported as owning these two checks in this replay, so they are filed here): 20261128T123106Z-ec2-emr-iam-a-cluster-reaches-waiting-but.md describes the same cluster-with-no-capacity symptom and names these two check ids in its trace list, but the replay that produced this report did not report it as owning them — the two checks reached no verdict again. 20261114T075942Z-ec2-emr-iam-a-cluster-built-on-an.md attributes the same two timeouts to the missing asynchronous TERMINATED_WITH_ERRORS / VALIDATION_ERROR settlement. Both halves are restated below as one cause because a single RunJobFlow lifecycle produces both observations; if either of those issues is fixed and these checks still time out, this one records that.

Cause 1 of 1: a cluster that RunJobFlow accepted is never given the Amazon EC2 instances its InstanceGroups asked for, and a cluster whose request names an unresolvable reference (a nonexistent Ec2SubnetId, or a ServiceRole that does not exist) is never settled into a terminal state either — so neither the capacity nor the terminal-failure observable ever appears.
Failing check ids for this cause: ec2__emr__iam.cluster-in-nonexistent-subnet-rejected, ec2__emr__iam.unknown-service-role-cluster-rejected.

  AWS: RunJobFlow accepts the request and resolves Ec2SubnetId / ServiceRole asynchronously, so a broken reference shows up only in the cluster's lifecycle. Smithy (com.amazonaws.emr): ClusterState = STARTING | BOOTSTRAPPING | RUNNING | WAITING | TERMINATING | TERMINATED | TERMINATED_WITH_ERRORS, and ClusterStateChangeReasonCode = INTERNAL_ERROR | VALIDATION_ERROR | INSTANCE_FAILURE | INSTANCE_FLEET_TIMEOUT | BOOTSTRAP_FAILURE | USER_REQUEST | STEP_FAILURE | ALL_STEPS_COMPLETED. Two requirements follow, and both checks depend on both: (a) a cluster that is NOT terminal has the nodes its InstanceGroups requested — ListInstances "provides information for all active Amazon EC2 instances" (active = AWAITING_FULFILLMENT | PROVISIONING | BOOTSTRAPPING | RUNNING per InstanceState) and returns one Instance per node carrying Ec2InstanceId, InstanceGroupId and Status.State, so a healthy cluster is observably backed by EC2 capacity; (b) a cluster whose request names a subnet or a service role that does not exist reaches DescribeCluster Status.State == TERMINATED_WITH_ERRORS with Status.StateChangeReason.Code == VALIDATION_ERROR and launches no EC2 capacity at all. Recorded behavior only for timings (a real multi-node cluster takes minutes).

  Emulator: neither observable is produced. Both checks begin with a positive control — RunJobFlow with a real subnet and real roles, then poll DescribeCluster for a terminal state and ListInstances for the first Ec2InstanceId, asserting ("launched", ids). The control cluster is accepted and never terminal, and ListInstances answers 200 with Instances = [] forever, so the control poll never satisfies. The broken-reference leg that follows (ghost subnet "subnet-0000000000000000f" / a never-created ServiceRole name) is never reached, and when it is reached it would poll for a TERMINAL state that is likewise never set. Both checks are therefore killed by the harness with `Error: Check timed out after 60.0s` and reach no verdict on the refusal they exist to test.
  Hypothesis: the cluster state machine advances (or parks) the cluster independently of whether capacity was actually provisioned, and the capacity launch contributes nothing — see services/service-com-amazonaws-emr-elasticmapreduce/src/state/cluster_instances.rs (launch_cluster_instances) and whatever drives post-RunJobFlow cluster state; the model says nothing about how capacity is provisioned, so this attribution is a hypothesis, while the two required observables above are the model-backed requirement.

  Why: a client that calls RunJobFlow gets a cluster that is neither usable nor failed. Automation that waits for nodes (ListInstances, DescribeCluster + EC2 DescribeInstances) blocks forever instead of getting capacity, and automation that waits for a bad reference to be rejected — the normal way an unresolvable Ec2SubnetId or ServiceRole is surfaced, since RunJobFlow itself returns 200 — never sees TERMINATED_WITH_ERRORS / VALIDATION_ERROR and so never learns the cluster is broken. Every downstream behavior that needs nodes (placement, bootstrap, steps, teardown) is masked as well.

  Fix: the emr service's cluster lifecycle and capacity launch — the post-RunJobFlow state machine plus services/service-com-amazonaws-emr-elasticmapreduce/src/state/cluster_instances.rs. A cluster must either be backed by the instances its InstanceGroups requested and report them through ListInstances, or be settled into TERMINATED_WITH_ERRORS with StateChangeReason.Code = VALIDATION_ERROR (and no EC2 capacity) when a reference in the request does not resolve. Presenting a non-terminal cluster with zero nodes is neither.

  Traces: ec2__emr__iam.cluster-in-nonexistent-subnet-rejected, ec2__emr__iam.unknown-service-role-cluster-rejected — each cut off at the harness budget inside its launched-capacity positive control, before its broken-reference assertion runs.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from this checkout. Service heading `### ec2__emr__iam`: 1/5 passed, 4 failed; two of the four were reported as owned by 20261128T123106Z-ec2-emr-iam-a-cluster-reaches-waiting-but.md and are skipped, the two below were not.

[mismatch] ec2__emr__iam.cluster-in-nonexistent-subnet-rejected
  Error: Check timed out after 60.0s

[mismatch] ec2__emr__iam.unknown-service-role-cluster-rejected
  Error: Check timed out after 60.0s

Neither produced a traceback: the harness cut the check off, so no assertion was reached.

Both checks run the same shape. First, the positive control:
  control_id = _launch(ctx, _params(ctx, service_role, profile, <real subnet>))
  got = _launched_or_failed(ctx, control_id)
  assert got[0] == "launched", f"control cluster {control_id} never launched capacity: {got!r}"
  assert _terminate(ctx, control_id) == "TERMINATED"
where _launched_or_failed polls DescribeCluster for a terminal state and otherwise reads
  sorted(i["Ec2InstanceId"] for i in emr.list_instances(ClusterId=...)["Instances"] if i.get("Ec2InstanceId"))
and returns ("launched", ids) only once that list is non-empty.

Then the broken-reference leg, which is never reached:
  ghost = "subnet-" + "0"*16 + "f"            # cluster-in-nonexistent-subnet-rejected
  missing = ctx.unique_name("sim2real-scn-emr-norole-")  # unknown-service-role-cluster-rejected
  bad_id = _launch(ctx, _params(ctx, ..., ghost|missing, ...))
  final = _poll(lambda: _state(ctx, bad_id), lambda s: s in TERMINAL_STATES, timeout_s=600.0, interval_s=5.0)
  reason = emr.describe_cluster(ClusterId=bad_id)["Cluster"]["Status"].get("StateChangeReason") or {}
  assert (final, reason.get("Code")) == ("TERMINATED_WITH_ERRORS", "VALIDATION_ERROR")
  assert _launched_ec2_ids(ctx, bad_id) == []

Expected: the control cluster lists its nodes (one Instance per requested node, Ec2InstanceId set, Status.State in AWAITING_FULFILLMENT/PROVISIONING/BOOTSTRAPPING/RUNNING), and the bad cluster reaches TERMINATED_WITH_ERRORS / VALIDATION_ERROR with no EC2 capacity.
Actual: ListInstances returns Instances = [] for the non-terminal control cluster and the cluster never becomes terminal, so the control poll never satisfies and the check is killed at the 60 s harness budget with no verdict.

## Steps to Reproduce
1. git clone the repo and let the behavioral suite build and boot the emulator from it.
2. Create a VPC and a private subnet; create an EMR service role (AmazonElasticMapReduceRole) and an EC2 instance profile (AmazonElasticMapReduceforEC2Role).
3. RunJobFlow with ReleaseLabel 'emr-6.15.0', Instances.InstanceGroups naming MASTER and CORE groups, Instances.Ec2SubnetId = the real subnet, JobFlowRole = the instance profile, ServiceRole = the role, KeepJobFlowAliveWhenNoSteps = true.
4. Poll DescribeCluster and ListInstances(ClusterId=<id>): expect at least one Instance with Ec2InstanceId set once the cluster leaves STARTING; observe Instances = [] indefinitely while the cluster is non-terminal.
5. RunJobFlow again with Instances.Ec2SubnetId = 'subnet-0000000000000000f' (a subnet that does not exist), then poll DescribeCluster: expect Status.State == 'TERMINATED_WITH_ERRORS' with Status.StateChangeReason.Code == 'VALIDATION_ERROR' and no EC2 capacity; observe no terminal state.
6. Repeat step 5 with ServiceRole set to a role name that was never created; expect the same TERMINATED_WITH_ERRORS / VALIDATION_ERROR settlement.
7. Replay ec2__emr__iam.cluster-in-nonexistent-subnet-rejected and ec2__emr__iam.unknown-service-role-cluster-rejected; both are currently killed at the 60 s harness budget inside the positive control of step 3-4.
