Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-catalog-describe-snapshots
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-catalog-describe-snapshots ec2-describe-images-self-owned-pagination-and-params ec2-pip-traj-instance-private-ip-reverse-lookup
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeSnapshots|2xx->2xx|$.DescribeSnapshotsResponse.snapshotSet
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-a1d7d3d1-1"].blockDeviceMapping.item.ebs.encrypted
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-a1d7d3d1-1"].freeTierEligible
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-a1d7d3d1-2"].blockDeviceMapping.item.ebs.encrypted
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-a1d7d3d1-2"].freeTierEligible
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-a1d7d3d1-3"].blockDeviceMapping.item.ebs.encrypted
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-a1d7d3d1-3"].freeTierEligible
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-a1d7d3d1-4"].blockDeviceMapping.item.ebs.encrypted
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-a1d7d3d1-4"].freeTierEligible
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-a1d7d3d1-5"].blockDeviceMapping.item.ebs.encrypted
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-a1d7d3d1-5"].freeTierEligible
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-a1d7d3d1-6"].blockDeviceMapping.item.ebs.encrypted
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-a1d7d3d1-6"].freeTierEligible
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-a1d7d3d1-7"].blockDeviceMapping.item.ebs.encrypted
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-a1d7d3d1-7"].freeTierEligible
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-a1d7d3d1-8"].blockDeviceMapping.item.ebs.encrypted
Issue-Key: ec2|DescribeImages|2xx->2xx|$.DescribeImagesResponse.imagesSet.item[name="sim2real-probe-ec2-img-a1d7d3d1-8"].freeTierEligible
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet
Issue-Commit: 588eab0496abf0a5bc1b8cec7adc2c989702bf8c

# ec2 read paths serve no catalog records and drop modeled members from describe projections (3 causes)

> _Note: this issue was identified against the code base at commit `588eab0496abf0a5bc1b8cec7adc2c989702bf8c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeSnapshots

## Description
Three ec2 describe paths answer HTTP 200 but with a payload that is not the shape AWS serves: a public snapshot catalog that is absent, image records missing modeled members, and an instance lookup whose reservationSet carries no matching reservation.

Cause 1 of 3: DescribeSnapshots serves no Amazon-published snapshot records.
AWS: recorded behavior only (the public snapshot catalog is data, not model). DescribeSnapshots for Amazon-owned snapshots answers HTTP 200 with snapshotSet containing an item LIST, each item carrying snapshotId, volumeId, status 'completed', startTime, progress '100%', ownerId, ownerAlias 'amazon', volumeSize, description, encrypted, storageTier, transferType, completionTime and fullSnapshotSizeInBytes. The individual ids, descriptions and timestamps are environment data, not a requirement; the requirement is that the filtered query returns a populated item list of snapshot records with those members.
Emulator: snapshotSet is a different type entirely (type_mismatch against the expected item list) - no snapshot records are served for the catalog query.
Why: anything that resolves a public/Amazon-owned snapshot (AMI block device backing, volume restore, snapshot-based tooling) sees an empty world and cannot proceed.
Fix: the ec2 snapshot catalog seed/state behind DescribeSnapshots and its snapshotSet serializer.
Traces: ec2-catalog-describe-snapshots#task#1 (cluster also covers #task#2).

Cause 2 of 3: DescribeImages drops ebs.encrypted and reports freeTierEligible false for self-owned AMIs.
AWS: recorded behavior only for these two members' values. DescribeImages answers HTTP 200 and each imagesSet item carries blockDeviceMapping.item.ebs.encrypted (serialized even when it is 'false') and freeTierEligible, which is 'true' for the recorded self-owned images.
Emulator: blockDeviceMapping.item.ebs.encrypted is omitted entirely for every image in the page, and freeTierEligible is reported 'false'.
Why: a client that reads image.BlockDeviceMappings[].Ebs.Encrypted gets a null it must guess at, and free-tier gating logic takes the wrong branch for every image.
Fix: the DescribeImages projection/serializer for blockDeviceMapping.ebs and the image record's freeTierEligible field in ec2 state.
Traces: ec2-describe-images-self-owned-pagination-and-params#task#15. (Open issues 20261112T182924Z-ec2-describeimages-omits-ebs-encrypted-and-reports-freetiereligible.md and 20261112T231741Z-ec2-describeimages-drops-blockdevicemapping-ebs-encrypted-and-reports.md describe the same family but carry neither this fixture's nor this step's replay key, so this step is not re-run by them.)

Cause 3 of 3: a DescribeInstances lookup by private IP returns no reservation.
AWS: recorded behavior only. DescribeInstances constrained to an instance's private IPv4 answers HTTP 200 with reservationSet carrying one item - reservationId, ownerId, groupSet and instancesSet.item for the matching instance (instanceId, instanceState running, privateIpAddress, subnetId, vpcId, networkInterfaceSet, tagSet, and so on).
Emulator: reservationSet is a different type from the expected single-item map (type_mismatch) - the matching reservation is not in the payload. Hypothesis: the private-IP constraint is not applied, so the set is serialized empty.
Why: reverse lookup of an instance by private address is the whole point of the call; a client gets no instance for an address it just launched.
Fix: the DescribeInstances handler's filter/lookup evaluation and its reservationSet serializer.
Traces: ec2-pip-traj-instance-private-ip-reverse-lookup#task#7.

## Evidence
Replay of trace set simparity-wire-aws-v1 (3 divergence clusters, verdict=mismatch, stub_path empty, owned_by empty):
1. ec2-catalog-describe-snapshots#task#1 DescribeSnapshots 200->200: $.DescribeSnapshotsResponse.snapshotSet type_mismatch - expected {'item': [{snapshotId, volumeId, status 'completed', startTime, progress '100%', ownerId, volumeSize, description, ownerAlias 'amazon', encrypted 'false', storageTier 'standard', transferType 'standard', completionTime, fullSnapshotSizeInBytes}, ...]}. Cluster also affects #task#2.
2. ec2-describe-images-self-owned-pagination-and-params#task#15 DescribeImages 200->200: for every imagesSet item, blockDeviceMapping.item.ebs.encrypted missing (expected 'false', got None) and freeTierEligible value_mismatch (expected 'true', got 'false').
3. ec2-pip-traj-instance-private-ip-reverse-lookup#task#7 DescribeInstances 200->200: $.DescribeInstancesResponse.reservationSet type_mismatch - expected {'item': {reservationId, ownerId, groupSet, instancesSet{item{instanceId, instanceState{code 16, name running}, privateIpAddress 10.238.1.10, subnetId, vpcId, networkInterfaceSet, tagSet, ...}}}}.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Replay ec2-catalog-describe-snapshots step task#1 (DescribeSnapshots over the Amazon-owned snapshot catalog) and inspect $.DescribeSnapshotsResponse.snapshotSet - it is not a populated item list.
3. Replay ec2-describe-images-self-owned-pagination-and-params step task#15 and inspect each imagesSet item's blockDeviceMapping.item.ebs.encrypted and freeTierEligible.
4. Replay ec2-pip-traj-instance-private-ip-reverse-lookup step task#7 (DescribeInstances constrained to the instance's private IPv4) and inspect $.DescribeInstancesResponse.reservationSet.
