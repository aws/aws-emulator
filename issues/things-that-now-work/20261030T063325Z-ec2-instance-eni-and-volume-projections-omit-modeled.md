Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-run-instances-validation-and-diagnostic-interrupt
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-run-instances-validation-and-diagnostic-interrupt ec2-page-describe-instances-tag-filtered-two-pages ec2-t12-eni-attach-and-private-ip ec2-conflict-volume-attachment
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|RunInstances|2xx->2xx|$.RunInstancesResponse.instancesSet.item.networkInterfaceSet.item.attachment.status
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.nextToken
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet
Issue-Key: ec2|DescribeNetworkInterfaces|2xx->2xx|$
Issue-Key: ec2|DescribeNetworkInterfaces|2xx->2xx|$.DescribeNetworkInterfacesResponse.networkInterfaceSet.item.attachment
Issue-Key: ec2|DescribeNetworkInterfaces|2xx->2xx|$.DescribeNetworkInterfacesResponse.networkInterfaceSet.item.attachment.status
Issue-Key: ec2|DescribeNetworkInterfaces|2xx->2xx|$.DescribeNetworkInterfacesResponse.networkInterfaceSet.item.status
Issue-Key: ec2|DescribeVolumes|2xx->4xx|$
Issue-Key: ec2|DescribeVolumes|2xx->4xx|$.DescribeVolumesResponse.volumeSet.item.status
Issue-Key: ec2|DescribeVolumes|2xx->4xx|status
Issue-Related: 20261023T102725Z-ec2-response-bodies-omit-modeled-members-and-report.md
Issue-Related: 20261030T000411Z-ec2-mutations-are-not-observable-reads-after-modify.md
Issue-Commit: 78d5907a1724d3304a3021142ffd4f12ae7136a6

# EC2 instance, ENI and volume projections omit modeled members and lose attachment state

> _Note: this issue was identified against the code base at commit `78d5907a1724d3304a3021142ffd4f12ae7136a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunInstances

## Description
Five net-new wire divergences share one root cause: the emulator's instance/ENI/volume projection is a reduced view of the modeled Instance, NetworkInterface and Volume structures - whole members are absent, and attachment/lifecycle transitions are either skipped or lost. This issue bundles 5 causes; all 5 must be closed.

Cause 1 of 5: RunInstances response omits most modeled instance members and reports the wrong attachment state.
AWS: Smithy: RunInstances returns Reservation (groupSet, instancesSet of Instance). The recording shows the launched instance carrying privateDnsName, dnsName (empty), productCodes (empty), operator, placement.availabilityZoneId / .groupName / .tenancy, groupSet with the security group, stateReason {code 'pending', message 'pending'}, rootDeviceType 'ebs', rootDeviceName '/dev/xvda', blockDeviceMapping, virtualizationType 'hvm', hypervisor 'xen', enaSupport, cpuOptions, capacityReservationSpecification, metadataOptions, enclaveOptions, maintenanceOptions, privateDnsNameOptions, bootMode/currentInstanceBootMode, and networkInterfaceSet.item.attachment.status 'attaching' at launch; the top-level Reservation also carries an empty groupSet.
Emulator: all of those members are absent from RunInstancesResponse, instancesSet.item.groupSet is serialized as an empty scalar rather than a list containing the group, and networkInterfaceSet.item.attachment.status is already 'attached' at launch.
Why: SDK clients read rootDeviceName, blockDeviceMapping, placement and cpuOptions straight off the RunInstances result; with those members absent they NPE or fall back to defaults, and an attachment that is 'attached' at launch removes the 'attaching' transition clients wait on.
Fix: the RunInstances response serializer/projection for the Instance structure.
Traces: ec2-run-instances-validation-and-diagnostic-interrupt#task#7.

Cause 2 of 5: DescribeInstances with a tag filter and pagination returns the wrong reservationSet shape and a wrong nextToken.
AWS: recorded behavior only for the page contents - the recorded page carries reservationSet.item as a list of reservations whose instancesSet items hold the full Instance projection (instanceState running, placement, blockDeviceMapping, networkInterfaceSet with attachment, tagSet), plus the paging nextToken.
Emulator: reservationSet has the wrong type/content for the page (type_mismatch against the recorded list) and the nextToken key diverges.
Why: a paginating client either loses instances or loops, and per-instance fields it needs are not in the page.
Fix: the DescribeInstances handler's filter+pagination projection.
Traces: ec2-page-describe-instances-tag-filtered-two-pages#task#12.

Cause 3 of 5: the next page of the same DescribeInstances sequence diverges the same way.
AWS: recorded behavior only - the second page carries the remaining reservations with the same full Instance projection.
Emulator: reservationSet type/content mismatch again.
Why: as cause 2 - the paged result set is not reproducible.
Fix: same component as cause 2.
Traces: ec2-page-describe-instances-tag-filtered-two-pages#task#14.

Cause 4 of 5: DescribeNetworkInterfaces never shows an attached ENI as in-use.
AWS: recorded behavior only - after AttachNetworkInterface the recording shows the ENI with status 'in-use', an attachment block (attachmentId, instanceId, instanceOwnerId, deviceIndex '1', networkCardIndex '0', status 'attached', attachTime, deleteOnTermination 'false'), publicDnsName (empty) and requesterId.
Emulator: the attachment block is absent, status stays out of 'in-use', and publicDnsName/requesterId are missing; the fixture's poll for the attached state capped at 60s.
Why: a client cannot discover which instance an ENI is attached to, nor detach it, because the attachmentId it needs is never returned.
Fix: the AttachNetworkInterface state mutation and the DescribeNetworkInterfaces projection of attachment/status.
Traces: ec2-t12-eni-attach-and-private-ip#task#8.

Cause 5 of 5: DescribeVolumes answers InvalidVolume.NotFound for a volume that still exists.
AWS: recorded behavior only - at this step the volume is still present and the recording answers HTTP 200 with volumeSet.item.status reaching 'available' (the fixture deletes it in a later step, which AWS answers 200).
Emulator: HTTP 400 with Code 'InvalidVolume.NotFound' and Message "The volume '<volumeId>' does not exist." (recorded run id: vol-0000000000000002d), so the poll for 'available' never converged (61 attempts).
Why: the volume's state is lost mid-lifecycle, so detach/delete sequences cannot be completed and teardown reports a leaked resource.
Fix: the volume state store / DescribeVolumes lookup after an attach-detach cycle.
Traces: ec2-conflict-volume-attachment#teardown#18.

## Evidence
Cause 1: $.RunInstancesResponse.groupSet missing (expected ''), instancesSet.item.privateDnsName / dnsName / productCodes / operator / placement.availabilityZoneId / placement.groupName / placement.tenancy / stateReason / rootDeviceType / rootDeviceName / blockDeviceMapping / virtualizationType / hypervisor / enaSupport / cpuOptions / capacityReservationSpecification / metadataOptions / enclaveOptions / maintenanceOptions / privateDnsNameOptions / bootMode / currentInstanceBootMode all missing; instancesSet.item.groupSet type_mismatch (expected the group list, got ''); networkInterfaceSet.item.attachment.status expected 'attaching', got 'attached'.
Cause 2/3: $.DescribeInstancesResponse.reservationSet type_mismatch against the recorded reservation list, plus $.DescribeInstancesResponse.nextToken divergence on task#12.
Cause 4: $.DescribeNetworkInterfacesResponse.networkInterfaceSet.item.attachment, .attachment.status, .status, .publicDnsName and .requesterId all diverge; poll capped at 60s.
Cause 5: status expected 200, got 400 with {'Code': 'InvalidVolume.NotFound', 'Message': "The volume 'vol-0000000000000002d' does not exist."}; $.DescribeVolumesResponse.volumeSet.item.status never reached 'available' after 61 attempts.

## Steps to Reproduce
Replay simparity-wire-aws-v1 and inspect ec2-run-instances-validation-and-diagnostic-interrupt#task#7, ec2-page-describe-instances-tag-filtered-two-pages#task#12 and #task#14, ec2-t12-eni-attach-and-private-ip#task#8 and ec2-conflict-volume-attachment#teardown#18. Diff each response against the recorded one: the first four are member-level omissions/state omissions on a 200, the last is a 400 NotFound where 200 was recorded.
