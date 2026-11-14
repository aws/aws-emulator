Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.ec2mr-byvpc-per-region-vpc-groups-are-disjoint-across-three-regions ec2.op-contract-modify-volume-attribute ec2.persist-hibernation-on-unencrypted-root-rejected
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.ec2mr-byvpc-per-region-vpc-groups-are-disjoint-across-three-regions|run#11|AssertionError|assert foreign == {}, (
Issue-Key: ec2.op-contract-modify-volume-attribute|run#12|AssertionError|assert code == "InvalidVolumeID.Malformed", f"unexpected error code: {code}"
Issue-Key: ec2.persist-hibernation-on-unencrypted-root-rejected|run#15|AssertionError|assert (code, message) == ("UnsupportedHibernationConfiguration", expected_message), (
Issue-Commit: a1e58af128a9f30f24b7da24f66fe92e314560f8

# EC2: 3 net-new behavioral root causes — DescribeVpcs is not region-scoped, ModifyVolumeAttribute misclassifies a fabricated volume id, and RunInstances with a NetworkInterfaces specification answers 501

> _Note: this issue was identified against the code base at commit `a1e58af128a9f30f24b7da24f66fe92e314560f8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeVpcs

## Description
Three net-new root causes in the ec2 crate, each grounded in its own failing behavioral check. They are bundled because they all belong to service `ec2`; a fixer must close ALL THREE before submitting.

=== Cause 1 of 3: EC2 resource state is not partitioned by region, so DescribeVpcs on one regional endpoint enumerates VPCs that were created through another region's endpoint. ===
Failing check id: ec2.ec2mr-byvpc-per-region-vpc-groups-are-disjoint-across-three-regions

AWS: EC2 is a regional service — each regional endpoint (ec2.<region>.amazonaws.com) owns its own resource namespace, and a VPC id minted in one region exists only in that region. Smithy (services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json): DescribeVpcs input is {Filters, VpcIds, NextToken, MaxResults (@range min 5 max 1000), DryRun}, output member `Vpcs` (VpcList, xmlName `vpcSet`), and `errors: []` — EC2 models no operation errors, and the model says nothing about regional partitioning, so this is recorded behavior only: against AWS, DescribeVpcs issued against us-east-1 returns only us-east-1 VPCs, and the same holds for DescribeInstances and every other region-scoped enumerate.
Emulator: the ec2 store is a single global map shared by all regions. The check created one VPC (plus one instance) in each of us-east-1, us-west-1 and us-west-2, then called DescribeVpcs on each regional endpoint; the us-east-1 call returned the other two regions' VPC ids as well. General behavior: any resource created through any region's endpoint is visible to reads on every other region's endpoint.
Why: every multi-region client is wrong against this emulator. A per-region inventory, tag sweep, compliance scan or quota count double- or triple-counts; a "delete every VPC in this region" runbook rehearsed on the emulator targets resources that live in other regions; CIDR-overlap and default-VPC assertions see foreign objects; and a region whose call is wrongly satisfied from another region's state hides a genuinely missing region.
Fix: the ec2 crate's state layer must be keyed by the request's endpoint/signing region so every read and write is scoped to it — services/service-com-amazonaws-ec2-amazonec2/src/state.rs (the store and its lookup helpers) and the enumerating handlers that read it, e.g. src/ops/describe_vpcs.rs and src/ops/describe_instances.rs.
Traces: ec2.ec2mr-byvpc-per-region-vpc-groups-are-disjoint-across-three-regions (assertion at check line 56).

=== Cause 2 of 3: ModifyVolumeAttribute classifies a fabricated-but-grammatical volume id as InvalidVolume.NotFound where AWS refuses it at id validation with InvalidVolumeID.Malformed. ===
Failing check id: ec2.op-contract-modify-volume-attribute

AWS: recorded behavior only — Smithy declares `errors: []` for ModifyVolumeAttribute (input: VolumeId required, AutoEnableIO, DryRun), so the refusal classes are EC2's own ec2Query codes rather than modeled shapes. AWS validates the volume id BEFORE any existence lookup: a fabricated documentation-style id such as `vol-1234567890abcdef0` is refused with HTTP 400 and code `InvalidVolumeID.Malformed`. The requirement is the class split, not one recording's id: an id EC2's grammar rejects is `InvalidVolumeID.Malformed` (malformed-class message `Invalid id: "<the id exactly as sent>"` — fixed prefix plus the rejected id from the request), and only an id that passes the grammar but names nothing is `InvalidVolume.NotFound`.
Emulator: answers code `InvalidVolume.NotFound`. services/service-com-amazonaws-ec2-amazonec2/src/ops/modify_volume_attribute.rs first calls `state::volume_id_is_well_formed` (lines 179-181), which accepts the id, and then resolves it with `state::require_volume_mut` (line 194), which reports absence — so the malformed branch is never taken for this id class.
Why: callers branch on Malformed (a permanent input bug — stop and fix the id) versus NotFound (the resource may not exist yet — create it and retry). Collapsing the malformed case into NotFound turns a permanent error into an apparently transient one, so retry/reconcile loops spin instead of failing fast.
Fix: ModifyVolumeAttribute's input validation and the shared volume-id grammar it consults (src/ops/modify_volume_attribute.rs plus the `volume_id_is_well_formed` definition in src/state.rs), so the grammar rejects ids outside EC2's volume id space before the store lookup runs. Related open issue that does NOT cover this check: 20261029T175114Z-ec2-client-error-classification-wrong-error-codes-for.md, whose Cause 7 records the identical `unexpected error code: InvalidVolume.NotFound` divergence for ModifyVolume (ec2.op-contract-modify-volume) but carries no replay key for ModifyVolumeAttribute.
Traces: ec2.op-contract-modify-volume-attribute (assertion at check line 52).

=== Cause 3 of 3: RunInstances with a NetworkInterfaces specification answers 501 NotImplemented, so the hibernation root-encryption refusal is unreachable and no subnet-pinned launch can be made. ===
Failing check id: ec2.persist-hibernation-on-unencrypted-root-rejected

AWS: RunInstances accepts a NetworkInterfaces list (InstanceNetworkInterfaceSpecification with DeviceIndex, SubnetId, Groups, AssociatePublicIpAddress) as an ordinary launch form — it is the only way to pin a subnet plus security groups and suppress a public IP in one request. Smithy declares `errors: []` for RunInstances, and HibernationOptionsRequest.Configured is documented as "Set to true to enable your instance for hibernation." Recorded AWS behavior: a RunInstances carrying HibernationOptions.Configured=true whose root BlockDeviceMapping has Ebs.Encrypted=false is refused with HTTP 400, code `UnsupportedHibernationConfiguration` and exactly the sentence "For hibernation, the root device volume must be encrypted." (the whole sentence is fixed text — it interpolates nothing from the request), and the rejected launch creates no instance and no volume; the same request with Encrypted=true is accepted and the instance describes with HibernationOptions.Configured=true.
Emulator: answers HTTP 501 with code `NotImplemented` and the emulator-authored message "operation RunInstances with a network interface specification is scaffolded but not implemented in this emulator". The hibernation/root-encryption validation is therefore never reached, and neither is the accepted-launch control path.
Why: a 501 on this launch form breaks every client that pins networking at launch (hibernate-capable workloads, private-subnet launches, ASG-style launches that pass Groups with a SubnetId), and the one refusal class clients catch to tell the user "encrypt the root volume" can never be produced. A caller cannot distinguish "the emulator does not implement this" from "AWS rejected my configuration".
Fix: the RunInstances handler's network-interface specification path (services/service-com-amazonaws-ec2-amazonec2/src/ops/run_instances.rs — the branch that currently returns the scaffolded NotImplemented for a request carrying NetworkInterfaces) and, behind it, the hibernation admission check that must refuse Configured=true over an unencrypted root with UnsupportedHibernationConfiguration before any instance or volume record is written. Related open issue that does NOT cover this check: 20261022T235108Z-ec2-sql-ha-application-status-check-fast-launch.md names the same 501 for ec2.ec2mr-inv2-* but carries no replay key for this check and does not name the hibernation validation behind it.
Traces: ec2.persist-hibernation-on-unencrypted-root-rejected (assertion at check line 60).

## Evidence
Replay diagnosis lines, verbatim, per cause.

Cause 1 — ec2.ec2mr-byvpc-per-region-vpc-groups-are-disjoint-across-three-regions, verdict [mismatch]:
`AssertionError: us-east-1 enumerated VPCs belonging to other regions: {'us-west-1': 'vpc-00000000000000031', 'us-west-2': 'vpc-0000000000000003f'} — DescribeVpcs is region-scoped and a shared global store is the failure this asserts against`
Expected (AWS): DescribeVpcs against the us-east-1 endpoint returns only VPCs created through the us-east-1 endpoint; the three regions yield three disjoint VPC groups and three disjoint instance sets. Actual (emulator): the us-east-1 enumeration contained the us-west-1 and us-west-2 VPC ids. The specific ids above are values from this run's state; the requirement is the disjointness, not the ids.

Cause 2 — ec2.op-contract-modify-volume-attribute, verdict [mismatch]:
`AssertionError: unexpected error code: InvalidVolume.NotFound` (assertion: `assert code == "InvalidVolumeID.Malformed"`).
Expected (AWS): ModifyVolumeAttribute(VolumeId="vol-1234567890abcdef0", AutoEnableIO={Value:true}) is refused 400 `InvalidVolumeID.Malformed` because the id fails EC2's id validation before any existence check runs; the malformed-class message is the fixed prefix `Invalid id: ` followed by the id exactly as sent. Actual (emulator): 400 with code `InvalidVolume.NotFound`. The earlier assertions in the same check passed, so the happy path (autoEnableIO flips false→true and DescribeVolumeAttribute reflects it) is correct — only the refusal classification diverges.

Cause 3 — ec2.persist-hibernation-on-unencrypted-root-rejected, verdict [mismatch]:
`AssertionError: RunInstances with HibernationOptions.Configured=true over an UNENCRYPTED root volume returned 'NotImplemented' ('operation RunInstances with a network interface specification is scaffolded but not implemented in this emulator'); real AWS refuses it with exactly 'UnsupportedHibernationConfiguration' ('For hibernation, the root device volume must be encrypted.')`
Expected (AWS): HTTP 400, code `UnsupportedHibernationConfiguration`, message "For hibernation, the root device volume must be encrypted." (fixed text), nothing created. Actual (emulator): HTTP 501, code `NotImplemented`, emulator-authored message naming the scaffolded network-interface path. The check had already confirmed via DescribeInstanceTypes that m5d.large reports HibernationSupported=true and sized the root volume above 2x RAM, so neither the instance type nor the volume size can be what is being refused.

## Steps to Reproduce
Boot the emulator and point an EC2 client at it.

Cause 1 (region scoping):
1. Against the us-west-2 endpoint, CreateVpc (e.g. 10.70.0.0/16) and note the VpcId; repeat against the us-west-1 endpoint (10.71.0.0/16) and the us-east-1 endpoint (10.72.0.0/16).
2. Call DescribeVpcs against the us-east-1 endpoint. Expected: only the us-east-1 VpcId appears. Actual: the us-west-1 and us-west-2 VpcIds appear too.
3. Same for DescribeInstances after launching one instance per region: each region must return only its own instance.
4. Re-replay ec2.ec2mr-byvpc-per-region-vpc-groups-are-disjoint-across-three-regions.

Cause 2 (ModifyVolumeAttribute id classification):
1. CreateVolume(Size=8, VolumeType="gp3", AvailabilityZone=<first zone DescribeAvailabilityZones reports>); wait for state `available`.
2. DescribeVolumeAttribute(VolumeId=<id>, Attribute="autoEnableIO") → Value false; ModifyVolumeAttribute(VolumeId=<id>, AutoEnableIO={Value:true}); DescribeVolumeAttribute again → Value true. (These already pass.)
3. ModifyVolumeAttribute(VolumeId="vol-1234567890abcdef0", AutoEnableIO={Value:true}). Expected: 400 `InvalidVolumeID.Malformed`. Actual: 400 `InvalidVolume.NotFound`.
4. Re-replay ec2.op-contract-modify-volume-attribute.

Cause 3 (RunInstances with NetworkInterfaces / hibernation):
1. CreateVpc + CreateSubnet + CreateSecurityGroup in a zone that offers m5d.large; resolve the newest AL2023 AMI and its root device name.
2. RunInstances(ImageId=<ami>, InstanceType="m5d.large", MinCount=1, MaxCount=1, HibernationOptions={Configured:true}, BlockDeviceMappings=[{DeviceName:<root device>, Ebs:{VolumeSize:<>= 2x RAM GiB + 20>, VolumeType:"gp3", Encrypted:false, DeleteOnTermination:true}}], NetworkInterfaces=[{DeviceIndex:0, SubnetId:<subnet>, Groups:[<sg>], AssociatePublicIpAddress:false}]). Expected: 400 `UnsupportedHibernationConfiguration`, "For hibernation, the root device volume must be encrypted.", and no instance or volume created. Actual: 501 `NotImplemented`, "operation RunInstances with a network interface specification is scaffolded but not implemented in this emulator".
3. Re-issue the same request with Ebs.Encrypted=true. Expected: the launch is accepted, the instance reaches `running`, DescribeInstances reports HibernationOptions.Configured=true, and the root volume describes Encrypted=true. Actual: the same 501.
4. Re-replay ec2.persist-hibernation-on-unencrypted-root-rejected.
