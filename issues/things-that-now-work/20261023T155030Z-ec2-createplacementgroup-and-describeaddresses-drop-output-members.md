Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-placement-group-and-tagging
Issue-Step: setup#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-placement-group-and-tagging ec2-t12-eip-disassociate-and-domain-attribute
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|CreatePlacementGroup|2xx->2xx|$.CreatePlacementGroupResponse.return
Issue-Key: ec2|CreatePlacementGroup|2xx->2xx|$headers.content-type
Issue-Key: ec2|DescribeAddresses|2xx->2xx|$.DescribeAddressesResponse.addressesSet.item.associationId
Issue-Key: ec2|DescribeAddresses|2xx->2xx|$.DescribeAddressesResponse.addressesSet.item.networkInterfaceId
Issue-Key: ec2|DescribeAddresses|2xx->2xx|$.DescribeAddressesResponse.addressesSet.item.networkInterfaceOwnerId
Issue-Key: ec2|DescribeAddresses|2xx->2xx|$.DescribeAddressesResponse.addressesSet.item.privateIpAddress
Issue-Related: 20261023T102725Z-ec2-responses-send-content-type-text-xml-without.md
Issue-Commit: 6d7833292dd9d4a45d8e52e2aa7ee7e5a822f7f5

# EC2 CreatePlacementGroup and DescribeAddresses drop output members

> _Note: this issue was identified against the code base at commit `6d7833292dd9d4a45d8e52e2aa7ee7e5a822f7f5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePlacementGroup

## Description
Two causes, both the same defect shape: a 200 response is serialized without members AWS always includes, so the caller cannot read a result it is entitled to.

Cause 1 of 2 — CreatePlacementGroup omits the ec2Query <return> element.
AWS: CreatePlacementGroupResponse carries return 'true'. Smithy: CreatePlacementGroup's output shape CreatePlacementGroupResult models only PlacementGroup (xmlName 'placementGroup'), so the <return> element is recorded behavior only — the recorded 200 for this step is CreatePlacementGroupResponse{requestId, return 'true'}.
Emulator: 200 with the element absent.
Trace: ec2-placement-group-and-tagging#setup#1.

Cause 2 of 2 — DescribeAddresses omits the association members after AssociateAddress.
AWS: once an Elastic IP is associated with a network interface, DescribeAddresses reports that address with associationId, networkInterfaceId, networkInterfaceOwnerId and privateIpAddress populated. Recorded behavior only (no error or state machine is modeled for DescribeAddresses).
Emulator: all four members are absent from the addressesSet item — the address is reported as if unassociated.
Trace: ec2-t12-eip-disassociate-and-domain-attribute#task#8.

Why: for cause 1, the ec2Query <return> element is how the SDKs populate the boolean result of a mutation; with it absent a caller cannot distinguish 'succeeded' from 'returned nothing' and any code that asserts on it fails on an operation that actually worked. For cause 2, associationId is the ONLY handle DisassociateAddress accepts for a VPC Elastic IP — a client that associates an address and then reads DescribeAddresses to find the association id to release it has nothing to pass, so the EIP can never be cleanly disassociated, and a client checking 'is this EIP attached?' concludes it is free and may re-associate it elsewhere.

Fix: the response serializers in the ec2 service crate — CreatePlacementGroup's handler/serializer must emit the <return> element, and DescribeAddresses' serializer must render the association members from the address state that AssociateAddress writes.

Related open issues: 20261023T102725Z-ec2-mutation-responses-omit-the-ec2query-return-element.md and 20261023T102725Z-ec2-response-bodies-omit-modeled-members-and-report.md describe the same families but carry neither of these two replay keys (owned_by: [] on both clusters), so they are filed here.

Traces: ec2-placement-group-and-tagging#setup#1 (representative), ec2-t12-eip-disassociate-and-domain-attribute#task#8.

## Evidence
Replay of simparity-wire-aws-v1, 2 divergence clusters, both status 200 expected and returned.

Cause 1 — divergence_key 'service=ec2|CreatePlacementGroup|status=200->200|keys=$.CreatePlacementGroupResponse.return,$headers.content-type', fixture ec2-placement-group-and-tagging step setup#1: "$.CreatePlacementGroupResponse.return: missing (expected 'true', got None)".

Cause 2 — divergence_key 'service=ec2|DescribeAddresses|status=200->200|keys=$.DescribeAddressesResponse.addressesSet.item.associationId,$.DescribeAddressesResponse.addressesSet.item.networkInterfaceId,$.DescribeAddressesResponse.addressesSet.item.networkInterfaceOwnerId,$.DescribeAddressesResponse.addressesSet.item.privateIpAddress,$headers.content-type', fixture ec2-t12-eip-disassociate-and-domain-attribute step task#8:
  "associationId: missing (expected '{{assoc_id}}', got None)"
  "networkInterfaceId: missing (expected '{{eni_id}}', got None)"
  "networkInterfaceOwnerId: missing (expected '000000000000', got None)"
  "privateIpAddress: missing (expected '{{eni_ip}}', got None)"
The expected values are fixture placeholders bound at replay time; the requirement asserted is that the four members be present and reflect the association, not any particular id.

## Steps to Reproduce
1. git_clone() then replay_wire_corpus().
2. Replay fixture ec2-placement-group-and-tagging, step setup#1 (CreatePlacementGroup): the 200 body is CreatePlacementGroupResponse with a requestId but no <return> element.
3. Replay fixture ec2-t12-eip-disassociate-and-domain-attribute to step task#8: after the EIP is associated with the ENI, DescribeAddresses returns the address with no associationId, networkInterfaceId, networkInterfaceOwnerId or privateIpAddress.
