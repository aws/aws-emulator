Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr.ebs-configuration-provisions-the-requested-real-volumes emr.core-instance-group-resize-adds-a-real-ec2-instance emr.emr-bootstrap-action-runs-once-on-every-provisioned-node emr.scaling-managed-scaling-headroom-adds-real-instances-for-pending-yarn-work
Issue-Shape-Id: com.amazonaws.emr#ElasticMapReduce
Issue-Key: emr.ebs-configuration-provisions-the-requested-real-volumes|_primary#10|KeyError|volumes = only["EbsVolumes"]
Issue-Key: emr.core-instance-group-resize-adds-a-real-ec2-instance|_poll#22|Timeout|time.sleep(interval_s)
Issue-Key: emr.emr-bootstrap-action-runs-once-on-every-provisioned-node|_poll#18|Timeout|time.sleep(interval_s)
Issue-Key: emr.scaling-managed-scaling-headroom-adds-real-instances-for-pending-yarn-work|run#30|Timeout|time.sleep(2)
Issue-Commit: 84939cc283904302bdfe02403fef158918b6c804

# EMR: 4 net-new root causes in the node-capacity plane — Instance.EbsVolumes is never serialized, a core-group resize adds no instance, bootstrap actions never run on a node, and managed scaling never grows capacity

> _Note: this issue was identified against the code base at commit `84939cc283904302bdfe02403fef158918b6c804`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListInstances

## Description
Four independent defects in how an EMR cluster's EC2 node capacity is provisioned and reported, each reached by its own behavioral check. THIS ISSUE BUNDLES 4 CAUSES. All four must be closed before submitting.

Cause 1 of 4: ListInstances serializes no EbsVolumes member, so the EBS volumes an EbsConfiguration asks for are never observable on the instance.
  AWS: an instance group created with EbsConfiguration.EbsBlockDeviceConfigs[{VolumeSpecification:{VolumeType, SizeInGB}, VolumesPerInstance: n}] has n EBS storage volumes attached to every node, and ListInstances reports each node with Instance.EbsVolumes — Smithy: Instance.EbsVolumes targets EbsVolumeList of EbsVolume{Device, VolumeId}, "The list of Amazon EBS volumes that are attached to this instance" — one entry per attached volume, each carrying a Device (e.g. /dev/sdh form) and the VolumeId EC2 DescribeVolumes resolves. ListInstanceGroups likewise reports InstanceGroup.EbsBlockDevices for that group.
  Emulator: the Instance structures ListInstances returns carry NO EbsVolumes key at all (the check's read of instance["EbsVolumes"] raises KeyError: 'EbsVolumes'), so neither a placeholder entry nor an attached volume is ever reported.
  Why: a client that reads a node's attached storage through EMR (capacity planning, verifying a requested volume layout, correlating EMR volumes with EC2 DescribeVolumes) sees a node with no storage; a modeled, always-present-on-AWS member is simply absent.
  Fix: the instance projection behind ListInstances (crate::state::ClusterInstanceRecord::to_instance in the emr service state, consumed by services/service-com-amazonaws-emr-elasticmapreduce/src/ops/list_instances.rs) plus RunJobFlow's handling of EbsConfiguration, which must record the per-instance volumes (device name + a real EC2 volume id) that the projection then serializes.
  Traces: emr.ebs-configuration-provisions-the-requested-real-volumes.

Cause 2 of 4: ModifyInstanceGroups does not grow a core instance group's real capacity.
  AWS: ModifyInstanceGroups(ClusterId, InstanceGroups=[{InstanceGroupId, InstanceCount: 2}]) on a RUNNING/WAITING cluster moves InstanceGroup.RequestedInstanceCount to the new target and EMR then launches the extra EC2 instance, so ListInstances for that group reports a second non-terminated instance (states AWAITING_FULFILLMENT/PROVISIONING/BOOTSTRAPPING/RUNNING) and RunningInstanceCount catches up. Smithy: InstanceGroup.RequestedInstanceCount / RunningInstanceCount; recorded behavior for the capacity arriving.
  Emulator: the check settles the pre-resize group over a 30 s freeze window, calls ModifyInstanceGroups, and then polls ListInstances for the group to reach 2 non-terminated instances — it never does within the budget, so the check reaches no verdict (the only output is 'Check timed out after 60.0s'). Hypothesis: the resize is recorded on the group but launches no instance record.
  Why: a client that scales a cluster out and waits for the capacity hangs forever and never gets the nodes it asked (and is billed) for.
  Fix: the ModifyInstanceGroups handler and the cluster's instance-group capacity logic in the emr service state — a raised InstanceCount must create the additional instance records (and lowering it must terminate them).
  Traces: emr.core-instance-group-resize-adds-a-real-ec2-instance.

Cause 3 of 4: a cluster's bootstrap actions never execute on the provisioned nodes.
  AWS: RunJobFlow with BootstrapActions=[{ScriptBootstrapAction:{Path: s3://...}}] runs that script exactly once on EVERY provisioned node (primary + core) during BOOTSTRAPPING, so the script's side effects — here one S3 object per node, keyed by the node's own IMDS instance id, with the role read from /mnt/var/lib/info/instance.json — exist for all 3 nodes once the cluster is live. Recorded behavior only (the model describes BootstrapActionConfig but not the execution).
  Emulator: no bootstrap action is executed on any node, so the per-node objects never appear and the check polls to the end of its budget without a verdict ('Check timed out after 60.0s').
  Why: every client whose cluster is configured by bootstrap actions (packages, config files, agents) gets nodes that were never configured, and the once-per-node contract cannot be observed at all.
  Fix: the bootstrap phase of the cluster lifecycle behind RunJobFlow (the per-node bootstrap-action runner and its node-side effects).
  Note: open issue 20261106T045334Z-emr-glue-s3-a-runjobflow-scriptbootstrapaction-stored-in.md reports the same mechanism under the emr__glue__s3 heading and does not carry this check id; open issue 20261029T184212Z-emr-clusters-accept-steps-but-never-execute-them.md covers STEP execution, not bootstrap actions on each node.
  Traces: emr.emr-bootstrap-action-runs-once-on-every-provisioned-node.

Cause 4 of 4: a ManagedScalingPolicy never adds capacity for pending YARN work.
  AWS: with ManagedScalingPolicy.ComputeLimits set, a cluster under pending YARN demand stays at the clamped MaximumCapacityUnits while the ceiling equals what is provisioned, and once the ceiling is raised EMR launches additional core/task instances (ListInstances shows more non-terminated instances; the primary is never scaled). Smithy: ManagedScalingPolicy.ComputeLimits — "The managed scaling activity of a cluster is not allowed to go above or below these limits ... only applies to the core and task nodes".
  Emulator: the cluster's node count never changes in either arm — no scaling activity is ever performed — so the check cannot distinguish the clamped window from the open one and burns its whole budget ('Check timed out after 60.0s'). Hypothesis: the policy is stored but no managed-scaling evaluator acts on it (the submitted Spark step also never runs, which open issue 20261108T151103Z-emr-4-net-new-root-causes-every-job.md covers for OTHER checks and must likely be fixed first for this one to grade).
  Why: a client relying on managed scaling gets a cluster that never grows under load and never shrinks after it, and cannot observe the ComputeLimits contract at all.
  Fix: the managed-scaling evaluator for a cluster carrying a ManagedScalingPolicy (PutManagedScalingPolicy/RunJobFlow state plus the cluster's capacity logic in the emr service state).
  Traces: emr.scaling-managed-scaling-headroom-adds-real-instances-for-pending-yarn-work.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from this checkout (emr: 119/220 passed).

Cause 1 — emr.ebs-configuration-provisions-the-requested-real-volumes:
  File "check_emr_ebs_configuration_provisions_the_requested_real_volumes.py", line 255, in _primary
    volumes = only["EbsVolumes"]
  KeyError: 'EbsVolumes'
The check requested EbsConfiguration with VolumesPerInstance=2 (gp2, 47 GiB) plus EbsRootVolumeSize=53 on a MASTER group, then paginated ListInstances(ClusterId, InstanceGroupTypes=['MASTER'], InstanceStates=[AWAITING_FULFILLMENT, PROVISIONING, BOOTSTRAPPING, RUNNING]). Expected: one Instance with EbsVolumes of length 2, every entry carrying Device and VolumeId (and ListInstanceGroups reporting 2 EbsBlockDevices). Actual: the Instance dict has no EbsVolumes member.

Cause 2 — emr.core-instance-group-resize-adds-a-real-ec2-instance: 'Error: Check timed out after 60.0s'. The check asserts the CORE group is settled at 1 instance over a 30 s freeze window, calls ModifyInstanceGroups(InstanceCount=2), then polls ListInstances for len(live ids) == 2. The growth never happens, so no verdict is reached.

Cause 3 — emr.emr-bootstrap-action-runs-once-on-every-provisioned-node: 'Error: Check timed out after 60.0s'. Expected 3 S3 objects under nodes/<instance-id> (one per node, written by the bootstrap script); none appears.

Cause 4 — emr.scaling-managed-scaling-headroom-adds-real-instances-for-pending-yarn-work: 'Error: Check timed out after 60.0s'. Expected the node count to stay at the clamped 2 units under pending demand and then grow after MaximumCapacityUnits is raised to 5; no capacity change is ever observed.

## Steps to Reproduce
1. git clone the repo and let the behavioral suite build and boot the emulator from it.
2. Cause 1: RunJobFlow a MASTER instance group with EbsConfiguration={EbsBlockDeviceConfigs:[{VolumeSpecification:{VolumeType:'gp2',SizeInGB:47},VolumesPerInstance:2}],EbsOptimized:true} and EbsRootVolumeSize=53, then call ListInstances(ClusterId, InstanceGroupTypes=['MASTER']) — expect each Instance to carry EbsVolumes with 2 entries (Device + VolumeId), observe no EbsVolumes member (replay emr.ebs-configuration-provisions-the-requested-real-volumes).
3. Cause 2: RunJobFlow with 1 primary + 1 core node, wait for the CORE group to settle, call ModifyInstanceGroups(ClusterId, [{InstanceGroupId: <core>, InstanceCount: 2}]) and poll ListInstances for that group — expect 2 non-terminated instances, observe 1 forever (replay emr.core-instance-group-resize-adds-a-real-ec2-instance).
4. Cause 3: RunJobFlow with BootstrapActions naming an S3 script that writes one object per node, then list that S3 prefix — expect one object per provisioned node, observe none (replay emr.emr-bootstrap-action-runs-once-on-every-provisioned-node).
5. Cause 4: RunJobFlow 1 primary + 1 core + 1 task with ManagedScalingPolicy ComputeLimits clamped to the provisioned units, submit a long pending Spark step, then raise MaximumCapacityUnits to 5 and poll ListInstances — expect capacity to grow only after the ceiling is raised, observe no change in either arm (replay emr.scaling-managed-scaling-headroom-adds-real-instances-for-pending-yarn-work).
