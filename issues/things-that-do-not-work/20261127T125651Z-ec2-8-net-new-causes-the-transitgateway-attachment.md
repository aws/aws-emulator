Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.tgw-attachment-nat-rules-apply-and-reset ec2.tgw-nat-rule-prefix-length-mismatch-rejected ec2.tgw-nat-rule-public-nat-cidr-rejected ec2.tgw-nat-rules-overlapping-nat-cidrs-rejected ec2.create-fleet-instant-tagged-delete-terminates ec2.deleting-a-network-interface-releases-its-elastic-ip ec2.nat-gateway-network-interface-is-listed-as-a-nat-gateway-interface ec2.instance-source-dest-check-change-is-visible-on-its-primary-interface ec2.describe-instance-types-maxresults-bounded-token-chain-spans-families ec2.interface-endpoint-regional-dns-name-resolves-to-its-network-interface-addresses ec2.ec2-interface-endpoint-private-dns-answers-regional-name-with-endpoint-eni-addresses ec2.ec2-instance-public-dns-name-resolves-to-private-address-in-vpc-and-public-address-outside ec2.imds-token-put-carrying-x-forwarded-for-rejected-while-the-plain-put-is-served
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|GetTransitGatewayAttachmentNatConfiguration|InvalidAction
Issue-Key: ec2.tgw-nat-rule-prefix-length-mismatch-rejected|run#19|AssertionError|assert got == want_refusal, f"refusal was {got!r}, expected {want_refusal!r}"
Issue-Key: ec2.tgw-nat-rule-public-nat-cidr-rejected|run#17|AssertionError|assert got == want_refusal, f"refusal was {got!r}, expected {want_refusal!r}"
Issue-Key: ec2.tgw-nat-rules-overlapping-nat-cidrs-rejected|run#17|AssertionError|assert got == want_refusal, f"refusal was {got!r}, expected {want_refusal!r}"
Issue-Key: ec2.create-fleet-instant-tagged-delete-terminates|run#22|AssertionError|assert labels.get("aws:ec2:fleet-id") == fleet_id, (
Issue-Key: ec2.deleting-a-network-interface-releases-its-elastic-ip|_poll#14|AssertionError|raise AssertionError(f"{what} not met within {budget_s}s; last value {value!r}")
Issue-Key: ec2.nat-gateway-network-interface-is-listed-as-a-nat-gateway-interface|run#18|AssertionError|assert addr["NetworkInterfaceId"] in listed, (
Issue-Key: ec2.instance-source-dest-check-change-is-visible-on-its-primary-interface|run#9|AssertionError|assert views == {k: False for k in views}, (
Issue-Key: ec2.describe-instance-types-maxresults-bounded-token-chain-spans-families|run#32|AssertionError|assert len(types4) == default_page, (
Issue-Key: ec2.interface-endpoint-regional-dns-name-resolves-to-its-network-interface-addresses|_poll#2|AssertionError|raise AssertionError(f"gave up after {budget:.0f}s waiting for {what}; last={value!r}")
Issue-Key: ec2.ec2-interface-endpoint-private-dns-answers-regional-name-with-endpoint-eni-addresses|_poll#20|Timeout|value = read()
Issue-Key: ec2.ec2-instance-public-dns-name-resolves-to-private-address-in-vpc-and-public-address-outside|_poll#20|Timeout|value = read()
Issue-Key: ec2.imds-token-put-carrying-x-forwarded-for-rejected-while-the-plain-put-is-served|_await_result#15|AssertionError|raise AssertionError(
Issue-Commit: ae5d8a902292096d9ed4c36fc40ed704ee301e98

# ec2: 8 net-new causes - the TransitGateway attachment NAT operations are unrouted, fleet/ENI/NAT-gateway state is not projected, DescribeInstanceTypes ignores the default page size, and the VPC resolver answers no endpoint DNS name

> _Note: this issue was identified against the code base at commit `ae5d8a902292096d9ed4c36fc40ed704ee301e98`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyTransitGatewayAttachmentNatConfiguration

## Description
Eight independent net-new causes in EC2 state projection, pagination, operation routing and the VPC data plane. Each must be closed before submitting.

Cause 1 of 8: the Transit Gateway attachment NAT-configuration operations are not routed at all.
AWS: recorded behavior only - `GetTransitGatewayAttachmentNatConfiguration` and `ModifyTransitGatewayAttachmentNatConfiguration` are absent from this repo's on-disk EC2 Smithy model (smithy_query reports 'operation not found' for both), so the model states nothing. The recordings show AWS serving them: a valid rule set applies and resets, and bad rule sets are refused with HTTP 400 `InvalidParameterValue` and these messages - `MatchSourceCidr and NatCidr must have the same prefix length, got /<match prefix> and /<nat prefix>`; `NatCidr '<cidr>' must be a private (RFC 1918) or CGNAT (RFC 6598) CIDR`; `NatCidr '<cidr>' overlaps with NatCidr '<cidr>'`. Fixed text is everything outside the prefix lengths and CIDRs, which come from the request.
Emulator: both actions are unknown to the EC2 Query dispatcher - `('InvalidAction', 'unknown action: ModifyTransitGatewayAttachmentNatConfiguration')` and `('InvalidAction', 'unknown action: GetTransitGatewayAttachmentNatConfiguration')`.
Why: a client cannot configure or read transit-gateway attachment NAT at all, and the InvalidAction it gets is indistinguishable from a typo in its own request rather than a missing feature.
Fix: route both operations in the EC2 operation dispatcher and add their handlers, with the NAT-rule validation above.
Traces: ec2.tgw-attachment-nat-rules-apply-and-reset, ec2.tgw-nat-rule-prefix-length-mismatch-rejected, ec2.tgw-nat-rule-public-nat-cidr-rejected, ec2.tgw-nat-rules-overlapping-nat-cidrs-rejected.

Cause 2 of 8: CreateFleet(type=instant) does not write the fleet's system tag onto the instances it launches.
AWS: recorded behavior only. Instances launched by a fleet carry the system tag `aws:ec2:fleet-id` whose value is the fleet id CreateFleet returned.
Emulator: DescribeInstances/DescribeTags report no tags at all on the launched instance (`its tags are {}`) although the fleet id exists.
Why: the fleet-id tag is how a client attributes an instance back to the fleet that made it and how fleet-scoped tag filters and cost allocation work; with no tag, instances launched by a fleet are indistinguishable from any other.
Fix: the CreateFleet handler's instance-launch path - stamp aws:ec2:fleet-id (alongside the request's TagSpecifications) on each launched instance.
Traces: ec2.create-fleet-instant-tagged-delete-terminates.

Cause 3 of 8: deleting a network interface does not release the Elastic IP associated with it.
AWS: recorded behavior only. DeleteNetworkInterface disassociates any Elastic IP on the interface; DescribeAddresses for the allocation then reports it unassociated - no AssociationId, no NetworkInterfaceId, no PrivateIpAddress.
Emulator: 30 s after the interface is deleted, DescribeAddresses still reports the address bound to the dead interface: AssociationId and NetworkInterfaceId still set, with NetworkInterfaceId naming an interface that no longer exists.
Why: the allocation is permanently wedged - a client cannot re-associate it (it reads as in use) and cannot reconcile it against a live interface, so the address leaks.
Fix: DeleteNetworkInterface's handler/state - cascade the address disassociation into the Elastic IP state.
Traces: ec2.deleting-a-network-interface-releases-its-elastic-ip.

Cause 4 of 8: a NAT gateway's network interface is not listed among its subnet's interfaces.
AWS: recorded behavior only. The ENI a NAT gateway reports in NatGatewayAddresses[].NetworkInterfaceId is a real interface and DescribeNetworkInterfaces filtered by that subnet returns it (InterfaceType `nat_gateway`).
Emulator: the NAT gateway reports NetworkInterfaceId `eni-000000000000000c7`, but DescribeNetworkInterfaces for the subnet returns `[]`.
Why: the id a client is handed cannot be read back, so NAT-gateway interfaces are invisible to every interface inventory, security-group audit and ENI-scoped tooling.
Fix: the NAT-gateway create path's state - register the gateway's interface in the network-interface store (InterfaceType nat_gateway) so DescribeNetworkInterfaces and its subnet filter return it.
Traces: ec2.nat-gateway-network-interface-is-listed-as-a-nat-gateway-interface.

Cause 5 of 8: ModifyInstanceAttribute's SourceDestCheck change is not propagated to the instance's primary network interface.
AWS: recorded behavior only. Disabling SourceDestCheck on an instance disables it on the instance's primary interface too, so all four views agree: DescribeInstances' instance-level SourceDestCheck, instance.NetworkInterfaces[0].SourceDestCheck, DescribeNetworkInterfaces for that ENI, and DescribeNetworkInterfaceAttribute(Attribute='sourceDestCheck').
Emulator: only the instance-level view flips - `{'instance': False, 'instance.NetworkInterfaces[0]': True, 'DescribeNetworkInterfaces': True, 'DescribeNetworkInterfaceAttribute': True}`.
Why: appliance/NAT-instance setups verify the flag on the interface; three of the four documented read paths still report checking enabled, so a client cannot confirm its own change and tooling that reads the interface concludes forwarding is still blocked.
Fix: ModifyInstanceAttribute's SourceDestCheck handler - write through to the primary ENI's attribute, which the three interface read paths project.
Traces: ec2.instance-source-dest-check-change-is-visible-on-its-primary-interface.

Cause 6 of 8: a DescribeInstanceTypes continuation sent without MaxResults returns every remaining item instead of the default page.
AWS: DescribeInstanceTypes is paginated (Smithy: inputToken/outputToken NextToken, items InstanceTypes, pageSize MaxResults) with MaxResults constrained to range min 5 / max 100. A request that omits MaxResults returns the service default page of 100 items with a further NextToken - a page can never exceed the modelled maximum of 100.
Emulator: the MaxResults-less continuation returns 1422 items in one page.
Why: a client paginating with no explicit page size gets the whole catalogue in one response - 14x the maximum page the model permits - breaking any consumer that sizes buffers, rate-limits or streams by page, and making the modelled MaxResults range meaningless.
Fix: DescribeInstanceTypes' pagination - apply the default page size (100) when MaxResults is absent, and cap any page at the modelled maximum.
Traces: ec2.describe-instance-types-maxresults-bounded-token-chain-spans-families.

Cause 7 of 8: the VPC Resolver answers no record for VPC-endpoint and instance DNS names.
AWS: recorded behavior only. The VPC's resolver answers an interface endpoint's Regional DNS name with the A records of the endpoint's network interfaces, answers the endpoint's private/service DNS name likewise from inside the VPC, and answers an instance's public DNS name with the private address inside the VPC and the public address outside it.
Emulator: the resolver returns a response with rcode 2 (SERVFAIL) and an empty answer section for the endpoint's Regional DNS name, polled to the budget - `{'id': 23834, 'response': True, 'rcode': 2, 'answers': []}`; the two sibling DNS checks never settle at all and are killed with `Check timed out after 60.0s`.
Why: every client that reaches an interface endpoint or an instance by name - which is the only supported way to use PrivateLink endpoints - fails to resolve, and a SERVFAIL (rather than NXDOMAIN or an answer) is retried by resolvers rather than reported.
Fix: the VPC resolver's zone data for VPC endpoints and instances - publish the endpoint's Regional and private DNS names onto its interface addresses, and the instance's public DNS name with split-horizon answers.
Traces: ec2.interface-endpoint-regional-dns-name-resolves-to-its-network-interface-addresses, ec2.ec2-interface-endpoint-private-dns-answers-regional-name-with-endpoint-eni-addresses, ec2.ec2-instance-public-dns-name-resolves-to-private-address-in-vpc-and-public-address-outside.

Cause 8 of 8: a guest's IMDS probe output never reaches the instance's console output.
AWS: recorded behavior only. User data that probes IMDS and echoes a line carrying the run's nonce appears in GetConsoleOutput, which is how the check reads the guest's verdict on the IMDS token PUT (the plain PUT served, the PUT carrying X-Forwarded-For refused).
Emulator: the nonce line never appears within the budget - `the guest's IMDS probe line for nonce '<nonce>' never reached the instance's console output within 30s`.
Why: a client cannot observe anything the guest wrote, so neither the IMDS behavior nor any user-data result is verifiable through the console.
Fix: the instance console-output state - carry the guest's stdout (user-data output) into GetConsoleOutput.
Traces: ec2.imds-token-put-carrying-x-forwarded-for-rejected-while-the-plain-put-is-served.

## Evidence
Cause 1 - expected ('InvalidParameterValue', 'MatchSourceCidr and NatCidr must have the same prefix length, got /24 and /25'), ('InvalidParameterValue', "NatCidr '198.51.100.0/24' must be a private (RFC 1918) or CGNAT (RFC 6598) CIDR"), ('InvalidParameterValue', "NatCidr '100.64.12.0/25' overlaps with NatCidr '100.64.12.0/25'") and a successful Get/Modify round trip; actual ('InvalidAction', 'unknown action: ModifyTransitGatewayAttachmentNatConfiguration') and ClientError (InvalidAction) ... GetTransitGatewayAttachmentNatConfiguration operation: unknown action.
Cause 2 - expected the launched instance to carry aws:ec2:fleet-id='fleet-00000000-0000-0000-0000-0000000000d0'; actual 'its tags are {}'.
Cause 3 - expected the allocation released from the deleted ENI; actual after 30s {'AllocationId': 'eipalloc-...c6', 'AssociationId': 'eipassoc-...c8', 'NetworkInterfaceId': 'eni-...c7', 'PrivateIpAddress': '10.207.0.4', 'PublicIp': '54.0.0.2', ...}.
Cause 4 - expected the NAT gateway interface among the subnet's interfaces; actual "NAT gateway interface 'eni-000000000000000c7' is not among the subnet's interfaces []".
Cause 5 - expected all four views False; actual {'instance': False, 'instance.NetworkInterfaces[0]': True, 'DescribeNetworkInterfaces': True, 'DescribeNetworkInterfaceAttribute': True}.
Cause 6 - expected the MaxResults-less continuation to return the default page of 100; actual 1422 items.
Cause 7 - expected A records of the endpoint's ENIs; actual {'id': 23834, 'response': True, 'rcode': 2, 'answers': []} after 51s, and 'Check timed out after 60.0s' for the two sibling DNS checks.
Cause 8 - expected the nonce line in GetConsoleOutput; actual "the guest's IMDS probe line for nonce 'd471a7d0ff914e4487d7fdfb31ae9b2c' never reached the instance's console output within 30s".

## Steps to Reproduce
Cause 1: create a transit gateway and a VPC attachment, then call ModifyTransitGatewayAttachmentNatConfiguration with a valid rule, with a prefix-length mismatch, with a public NatCidr and with overlapping NatCidrs, and call GetTransitGatewayAttachmentNatConfiguration -> observe InvalidAction for both actions.
Cause 2: CreateFleet(Type='instant', TargetCapacitySpecification=1) over a launch template; DescribeTags/DescribeInstances for the launched instance -> no aws:ec2:fleet-id.
Cause 3: AllocateAddress; CreateNetworkInterface; AssociateAddress onto it; DeleteNetworkInterface; poll DescribeAddresses(AllocationIds=[alloc]) -> still associated.
Cause 4: CreateNatGateway in a public subnet; read NatGatewayAddresses[].NetworkInterfaceId; DescribeNetworkInterfaces(Filters=[{'Name':'subnet-id','Values':[subnet]}]) -> empty.
Cause 5: launch an instance; ModifyInstanceAttribute(SourceDestCheck={'Value': False}); read SourceDestCheck via DescribeInstances (instance and NetworkInterfaces[0]), DescribeNetworkInterfaces and DescribeNetworkInterfaceAttribute -> only the instance view flips.
Cause 6: DescribeInstanceTypes(MaxResults=5) three times following NextToken, then one more call with the NextToken and no MaxResults -> 1422 items.
Cause 7: create an interface VPC endpoint with private DNS, then query the VPC resolver for the endpoint's Regional DNS name and for the instance's public DNS name from inside and outside the VPC -> rcode 2 / no answers.
Cause 8: RunInstances with user data that PUTs /latest/api/token with and without X-Forwarded-For and echoes a nonce-tagged line; poll GetConsoleOutput -> the line never appears.
