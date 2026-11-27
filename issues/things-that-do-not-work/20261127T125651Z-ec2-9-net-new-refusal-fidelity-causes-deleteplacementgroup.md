Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.placement-group-with-a-running-member-refuses-deletion ec2.run-instances-both-availability-zone-and-zone-id-rejected ec2.run-instances-subnet-zone-mismatched-availability-zone-rejected ec2.stopped-x86-instance-refuses-an-arm64-instance-type ec2.security-group-swapped-off-a-primary-interface-becomes-deletable ec2.keypair-create-tags-nonexistent-id-rejected ec2.keypair-delete-tags-nonexistent-id-succeeds ec2.dpeer-overlapping-cidr-peering-fails-and-never-carries-traffic ec2.dpeer-route-is-blackhole-until-the-peering-is-accepted
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.placement-group-with-a-running-member-refuses-deletion|_error#7|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: ec2.run-instances-both-availability-zone-and-zone-id-rejected|_error#9|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: ec2.run-instances-subnet-zone-mismatched-availability-zone-rejected|_error#11|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: ec2.stopped-x86-instance-refuses-an-arm64-instance-type|_error#7|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: ec2|DeleteSecurityGroup|DependencyViolation
Issue-Key: ec2.keypair-create-tags-nonexistent-id-rejected|run#8|AssertionError|assert message == expected_message, f"unexpected message {message!r}"
Issue-Key: ec2|DeleteTags|InvalidKeyPair.NotFound
Issue-Key: ec2.dpeer-overlapping-cidr-peering-fails-and-never-carries-traffic|_leg1_overlapping_peering_fails#5|AssertionError|assert created["Status"]["Code"] == "initiating-request", (
Issue-Key: ec2.dpeer-route-is-blackhole-until-the-peering-is-accepted|_assert_route_state#6|AssertionError|assert route.get("State") == want_state, (
Issue-Commit: ae5d8a902292096d9ed4c36fc40ed704ee301e98

# ec2: 9 net-new refusal-fidelity causes - DeletePlacementGroup/DeleteSecurityGroup/ModifyInstanceAttribute/RunInstances accept or refuse the wrong way, and VPC-peering status and route state skip their real values

> _Note: this issue was identified against the code base at commit `ae5d8a902292096d9ed4c36fc40ed704ee301e98`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeletePlacementGroup

## Description
Nine independent net-new causes in EC2's refusal and state-transition fidelity. Each must be closed before submitting.

Cause 1 of 9: DeletePlacementGroup succeeds while a running instance is still a member.
AWS: DeletePlacementGroup (Smithy: input GroupName, required; output Unit; no modelled errors - EC2 Query reports refusals as common errors) refuses a group that still holds an instance with HTTP 400 `InvalidPlacementGroup.InUse` and the message `The placement group '<group name>' is in use and may not be deleted.` - fixed text `The placement group '` / `' is in use and may not be deleted.`, with the group name taken from the request.
Emulator: answers HTTP 200 with an empty DeletePlacementGroupResult.
Why: a client that relies on the refusal to protect a group with live members silently loses it, and sees no error to retry after terminating members.
Fix: DeletePlacementGroup's handler/validation - refuse while any non-terminated instance has Placement.GroupName equal to the group.
Traces: ec2.placement-group-with-a-running-member-refuses-deletion.

Cause 2 of 9: RunInstances accepts Placement.AvailabilityZone together with Placement.AvailabilityZoneId.
AWS: recorded behavior only (the model marks neither member exclusive). AWS refuses the pair with `InvalidParameterCombination` and the fixed message `AvailabilityZone and AvailabilityZoneId can't be used together. Specify only one of these parameters.`
Emulator: launches the instance and returns a Reservation (HTTP 200).
Why: a client that names both fields gets an instance placed by whichever field the emulator happens to read, with no signal that its request was ambiguous.
Fix: RunInstances' input validation of the Placement structure.
Traces: ec2.run-instances-both-availability-zone-and-zone-id-rejected.

Cause 3 of 9: RunInstances accepts a Placement.AvailabilityZone that disagrees with the SubnetId's zone.
AWS: recorded behavior only. AWS refuses with `InvalidParameterValue` and the message `Value (<requested zone>) for parameter availabilityZone is invalid. Subnet '<subnet id>' is in the availability zone <subnet's zone>` - fixed text `Value (`, `) for parameter availabilityZone is invalid. Subnet '`, `' is in the availability zone `; the requested zone, the subnet id and the subnet's own zone come from the request and from state. The refused launch must leave the VPC empty.
Emulator: launches the instance (HTTP 200, Reservation returned) in the subnet.
Why: a client that mis-pairs subnet and zone gets a silently relocated instance instead of the refusal it codes against, and the VPC is left holding an instance the client believes was never created.
Fix: RunInstances' placement validation - cross-check Placement.AvailabilityZone against the named subnet's zone.
Traces: ec2.run-instances-subnet-zone-mismatched-availability-zone-rejected.

Cause 4 of 9: ModifyInstanceAttribute accepts an arm64 instance type on an x86_64 instance.
AWS: recorded behavior only. For a stopped x86_64 instance, setting InstanceType to an arm64 type is refused with `InvalidInstanceAttributeValue` and the message `'<requested type>' is not a valid instance type for instance '<instance id>' of architecture '<architecture>'.` - fixed text `' is not a valid instance type for instance '` / `' of architecture '` / `'.`; the type, instance id and architecture come from the request and state. The instance keeps its original type and architecture.
Emulator: answers HTTP 200 and accepts the change.
Why: a client resizing an instance across architectures gets an instance whose type and AMI architecture disagree instead of the documented refusal (see AWS's resize-limitations guidance).
Fix: ModifyInstanceAttribute's InstanceType validation against the instance's architecture.
Traces: ec2.stopped-x86-instance-refuses-an-arm64-instance-type.

Cause 5 of 9: DeleteSecurityGroup still reports DependencyViolation after the group was swapped off the only interface using it.
AWS: recorded behavior only. Once ModifySecurityGroupRules/ModifyNetworkInterfaceAttribute has replaced the group on the ENI, the group has no dependent object and DeleteSecurityGroup succeeds. AWS's DependencyViolation (`resource <group id> has a dependent object`) is correct only while something still references the group.
Emulator: refuses with `('DependencyViolation', 'resource <group id> has a dependent object')` for a group no interface carries any more.
Why: a client that detaches a group before deleting it can never delete it; the group leaks for the account's lifetime.
Fix: the security-group dependency index consulted by DeleteSecurityGroup - it is not cleared when a group is swapped off a network interface.
Traces: ec2.security-group-swapped-off-a-primary-interface-becomes-deletable.

Cause 6 of 9: CreateTags' not-found message for a deleted key-pair id is worded wrong.
AWS: the refusal is `InvalidKeyPair.NotFound` with the message `The key-pair ID '<key pair id>' does not exist` - the fixed text uses the hyphenated noun `key-pair ID`; the id comes from the request. (DescribeKeyPairs for the same id uses a different fixed noun, `The keyPairId '<id>' does not exist`, so the two messages are not interchangeable.)
Emulator: answers the right code but the message `The key pair ID 'key-000000000000000aa' does not exist` - `key pair ID` with a space instead of `key-pair ID`.
Why: a client matching on the message text to distinguish CreateTags' key-pair refusal from DescribeKeyPairs' does not match.
Fix: the CreateTags handler's key-pair not-found message template.
Traces: ec2.keypair-create-tags-nonexistent-id-rejected.

Cause 7 of 9: DeleteTags refuses a key-pair id that no longer exists.
AWS: DeleteTags (Smithy: input Resources required, Tags, DryRun; output Unit; no modelled errors) is idempotent over resources - deleting tags naming an id that is already gone succeeds with HTTP 200 and an empty result.
Emulator: refuses with HTTP 400 `InvalidKeyPair.NotFound` / `The key pair ID 'key-000000000000000aa' does not exist`.
Why: cleanup code that calls DeleteTags unconditionally - the normal teardown idiom precisely because DeleteTags tolerates missing ids - now raises, so teardown aborts part-done.
Fix: DeleteTags' handler - do not resolve ids for existence; drop tags for the ids present and succeed.
Traces: ec2.keypair-delete-tags-nonexistent-id-succeeds.

Cause 8 of 9: CreateVpcPeeringConnection returns Status.Code `failed` synchronously for overlapping CIDRs.
AWS: recorded behavior only. The request is accepted and the connection is returned with Status.Code `initiating-request`; the overlap verdict is reached asynchronously and only then becomes `failed` with Message `Failed due to incorrect VPC-ID, Account ID, or overlapping CIDR range`.
Emulator: the create call itself already returns `{'Code': 'failed', 'Message': 'Failed due to incorrect VPC-ID, Account ID, or overlapping CIDR range'}`.
Why: a client that reads the create response's status and then polls for the transition sees no `initiating-request` state at all, so the state machine it codes against (accept, then fail) is unobservable and a client that treats a non-`failed` create as "pending acceptance" mis-branches.
Fix: the CreateVpcPeeringConnection handler and the peering-connection state machine - accept into initiating-request and settle the overlap verdict afterwards.
Traces: ec2.dpeer-overlapping-cidr-peering-fails-and-never-carries-traffic.

Cause 9 of 9: a route targeting a not-yet-accepted VPC peering connection reads State `active` instead of `blackhole`.
AWS: recorded behavior only. A route whose VpcPeeringConnectionId names a peering that is still pending acceptance has State `blackhole`; it becomes `active` only once the peering is accepted.
Emulator: DescribeRouteTables reports `{'DestinationCidrBlock': '10.64.0.0/16', 'Origin': 'CreateRoute', 'State': 'active', 'VpcPeeringConnectionId': 'pcx-...'}` while the peering is still pending.
Why: State is how a client tells a usable peering route from a dead one; reported as active, a client concludes traffic will cross a peering that is not accepted.
Fix: the route-table state projection - derive a peering route's State from the referenced peering connection's status.
Traces: ec2.dpeer-route-is-blackhole-until-the-peering-is-accepted.

## Evidence
Cause 1 - expected ('InvalidPlacementGroup.InUse', "The placement group '<name>' is in use and may not be deleted."); actual HTTP 200 {'ResponseMetadata': {'HTTPStatusCode': 200, ...}} -> AssertionError: expected the call to be refused, but it succeeded.
Cause 2 - expected ('InvalidParameterCombination', "AvailabilityZone and AvailabilityZoneId can't be used together. Specify only one of these parameters."); actual HTTP 200 with ReservationId 'r-00000000000000001' and a pending instance.
Cause 3 - expected ('InvalidParameterValue', "Value (<zone_b>) for parameter availabilityZone is invalid. Subnet '<subnet>' is in the availability zone <zone_a>"); actual HTTP 200 with ReservationId 'r-00000000000000001' launched in the subnet.
Cause 4 - expected ('InvalidInstanceAttributeValue', "'t4g.micro' is not a valid instance type for instance '<id>' of architecture 'x86_64'."); actual HTTP 200 and the change applied.
Cause 5 - expected the delete of the detached group to succeed; actual ('DependencyViolation', 'resource sg-000000000000000b4 has a dependent object').
Cause 6 - expected "The key-pair ID 'key-...' does not exist"; actual "The key pair ID 'key-000000000000000aa' does not exist".
Cause 7 - expected DeleteTags to succeed; actual botocore ClientError (InvalidKeyPair.NotFound) when calling the DeleteTags operation: The key pair ID 'key-000000000000000aa' does not exist.
Cause 8 - expected Status {'Code': 'initiating-request'}; actual {'Code': 'failed', 'Message': 'Failed due to incorrect VPC-ID, Account ID, or overlapping CIDR range'} from CreateVpcPeeringConnection itself.
Cause 9 - expected the route to 10.64.0.0/16 to be State 'blackhole' while the peering is pending; actual {'DestinationCidrBlock': '10.64.0.0/16', 'Origin': 'CreateRoute', 'State': 'active', 'VpcPeeringConnectionId': 'pcx-000000000000000e1'}.

## Steps to Reproduce
Cause 1: CreatePlacementGroup(Strategy=spread); RunInstances into it; wait running; DeletePlacementGroup -> expect InvalidPlacementGroup.InUse, observe 200.
Cause 2: RunInstances with Placement={'AvailabilityZone': z, 'AvailabilityZoneId': zid} -> expect InvalidParameterCombination, observe a launch.
Cause 3: CreateSubnet in zone A; RunInstances with that SubnetId and Placement={'AvailabilityZone': zone B} -> expect InvalidParameterValue, observe a launch.
Cause 4: launch a t3.micro from the x86_64 AL2023 AMI, StopInstances, wait stopped, ModifyInstanceAttribute(InstanceType={'Value':'t4g.micro'}) -> expect InvalidInstanceAttributeValue, observe 200.
Cause 5: create two security groups, create an ENI with group A, ModifyNetworkInterfaceAttribute to group B, DeleteSecurityGroup(A) -> expect success, observe DependencyViolation.
Cause 6/7: CreateKeyPair, CreateTags on its id, DeleteKeyPair, then CreateTags on the dead id (check the message wording) and DeleteTags on the dead id (expect success).
Cause 8: create two VPCs with overlapping CIDRs and CreateVpcPeeringConnection between them; read Status.Code in the create response.
Cause 9: create two non-overlapping VPCs, CreateVpcPeeringConnection, do NOT accept it, CreateRoute in the requester's route table targeting the pcx, then DescribeRouteTables and read the route's State.
