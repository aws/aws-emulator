Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-associate-vpc-cidr-block-restricted-range-on-rfc1918-block
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-associate-vpc-cidr-block-restricted-range-on-rfc1918-block ec2-ipam-consolidated-delete-and-deprovision-lifecycle ec2-instance-tag-change-and-remove-last-then-terminated
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|AssociateVpcCidrBlock|2xx->2xx|$.AssociateVpcCidrBlockResponse.cidrBlockAssociation.cidrBlockState.state
Issue-Key: ec2|DescribeVpcs|2xx->2xx|$.DescribeVpcsResponse.vpcSet.item.cidrBlockAssociationSet.item[*]
Issue-Key: ec2|DescribeVpcs|2xx->2xx|$.DescribeVpcsResponse.vpcSet.item.cidrBlockAssociationSet.item[*].cidrBlockState.state
Issue-Key: ec2|DeleteIpamPrefixListResolverTarget|2xx->2xx|$.DeleteIpamPrefixListResolverTargetResponse.ipamPrefixListResolverTarget.desiredVersion
Issue-Key: ec2|DeleteIpamPrefixListResolver|2xx->2xx|$.DeleteIpamPrefixListResolverResponse.ipamPrefixListResolver.lastVersionCreationStatus
Issue-Key: ec2|DeleteIpamPrefixListResolver|2xx->2xx|$.DeleteIpamPrefixListResolverResponse.ipamPrefixListResolver.lastVersionCreationStatusMessage
Issue-Key: ec2|DescribeInstances|2xx->2xx|$.DescribeInstancesResponse.reservationSet.item.instancesSet.item.tagSet
Issue-Commit: ba97c8568f94e8f0e1657393f851b27fdc51a8e0

# ec2 omits response members and never projects association state through its lifecycle

> _Note: this issue was identified against the code base at commit `ba97c8568f94e8f0e1657393f851b27fdc51a8e0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateVpcCidrBlock

## Description
Five responses diverge in the members and state values the emulator projects: a mutation reports its terminal state immediately instead of the transitional one, a disassociated association never reaches its terminal state, two delete responses drop members AWS always echoes, and an empty collection is emitted where AWS omits the member. This is a BUNDLE OF 5 CAUSES; all five must be closed.

Cause 1 of 5: AssociateVpcCidrBlock reports cidrBlockState.state 'associated' on the mutation response.
AWS: Smithy: VpcCidrBlockState.State targets the VpcCidrBlockStateCode enum, values associating | associated | disassociating | disassociated | failing | failed. The recording shows the mutation response carrying 'associating'; 'associated' is reached only on a later read.
Emulator: the mutation response carries 'associated'.
Why: a client that waits for associating -> associated to confirm the association has nothing to observe, and code that asserts the transitional state on the call's own response fails.
Fix: AssociateVpcCidrBlock's response projection and the CIDR-association state machine — return the transitional state from the mutation and settle to 'associated' on subsequent reads.
Traces: ec2-associate-vpc-cidr-block-restricted-range-on-rfc1918-block#task#4.

Cause 2 of 5: after a disassociation, DescribeVpcs never reports the association as 'disassociated'.
AWS: Smithy: the same VpcCidrBlockStateCode enum includes 'disassociated'. The recording shows the disassociated association remaining in vpcSet.item.cidrBlockAssociationSet with cidrBlockState.state 'disassociated'.
Emulator: the association's state stays 'associated' through a 30-attempt poll, and the association set carries extra rows instead of the recorded one.
Why: a client polling for 'disassociated' never terminates (the replay records the poll exhausting 30 attempts), so teardown cannot be confirmed.
Fix: the VPC CIDR-association state projection read by DescribeVpcs — record the disassociation and surface the terminal state.
Traces: ec2-associate-vpc-cidr-block-restricted-range-on-rfc1918-block#task#7.

Cause 3 of 5: DeleteIpamPrefixListResolverTarget omits desiredVersion from the deleted resource it echoes.
AWS: recorded behavior only. The delete response echoes the target structure including desiredVersion.
Emulator: ipamPrefixListResolverTarget.desiredVersion is absent.
Why: a client that reads the deleted resource's version off the delete response (to log or to reconcile) gets a null field.
Fix: DeleteIpamPrefixListResolverTarget's response serializer — echo the full stored target structure.
Traces: ec2-ipam-consolidated-delete-and-deprovision-lifecycle#task#30.

Cause 4 of 5: DeleteIpamPrefixListResolver omits lastVersionCreationStatus and lastVersionCreationStatusMessage.
AWS: recorded behavior only. The delete response echoes the resolver structure including lastVersionCreationStatus ('success') and lastVersionCreationStatusMessage, whose text names the version that was created ('Successfully created version <n>.' — fixed text 'Successfully created version ' and '.', variable part the version number from the resolver's state).
Emulator: both members are absent.
Why: the resolver's last-version outcome is unreadable from the delete response, so a client cannot tell whether the resolver it deleted had successfully produced a version.
Fix: DeleteIpamPrefixListResolver's response serializer and the resolver state that must retain the last-version status/message.
Traces: ec2-ipam-consolidated-delete-and-deprovision-lifecycle#task#32.

Cause 5 of 5: DescribeInstances emits an empty tagSet for an instance with no tags.
AWS: recorded behavior only. Once the last tag is removed, AWS omits the instance's tagSet member entirely rather than serialising it empty.
Emulator: tagSet is present and empty (reported unexpected_present).
Why: a client that treats the presence of tagSet as 'this instance has tags' mis-reads an untagged instance, and strict response comparisons diverge.
Fix: the DescribeInstances response serializer — omit empty collection members instead of emitting an empty element.
Traces: ec2-instance-tag-change-and-remove-last-then-terminated#task#11.

## Evidence
Cause 1 — AssociateVpcCidrBlock 200/200: $.AssociateVpcCidrBlockResponse.cidrBlockAssociation.cidrBlockState.state expected 'associating', got 'associated'.
Cause 2 — DescribeVpcs 200/200: cidrBlockAssociationSet.item[1] expected {'cidrBlock': '10.120.0.0/16', 'associationId': 'vpc-cidr-assoc-...cc', 'cidrBlockState': {'state': 'disassociated'}} but served {'cidrBlock': '192.168.0.0/16', 'cidrBlockState': {'state': 'associated'}}; items [2] and [3] extra; and 'poll: not in [disassociated] after 30 attempt(s) (expected disassociated, got associated)'.
Cause 3 — DeleteIpamPrefixListResolverTarget 200/200: $.DeleteIpamPrefixListResolverTargetResponse.ipamPrefixListResolverTarget.desiredVersion missing (expected '1', got None).
Cause 4 — DeleteIpamPrefixListResolver 200/200: lastVersionCreationStatus missing (expected 'success', got None); lastVersionCreationStatusMessage missing (expected 'Successfully created version 1.', got None).
Cause 5 — DescribeInstances 200/200: $.DescribeInstancesResponse.reservationSet.item.instancesSet.item.tagSet unexpected_present (expected None, got '').
The association ids, CIDRs and version numbers are the recordings' values; the requirement is the state sequence and the member presence.

## Steps to Reproduce
1. Boot the emulator and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Cause 1: ec2-associate-vpc-cidr-block-restricted-range-on-rfc1918-block step task#4 — AssociateVpcCidrBlock and read cidrBlockState.state off the mutation response.
3. Cause 2: same fixture step task#7 — after disassociating a CIDR, poll DescribeVpcs for cidrBlockAssociationSet state 'disassociated'.
4. Cause 3: ec2-ipam-consolidated-delete-and-deprovision-lifecycle step task#30 — DeleteIpamPrefixListResolverTarget and check desiredVersion.
5. Cause 4: same fixture step task#32 — DeleteIpamPrefixListResolver and check lastVersionCreationStatus/Message.
6. Cause 5: ec2-instance-tag-change-and-remove-last-then-terminated step task#11 — remove an instance's last tag, then DescribeInstances and check whether tagSet is emitted.
