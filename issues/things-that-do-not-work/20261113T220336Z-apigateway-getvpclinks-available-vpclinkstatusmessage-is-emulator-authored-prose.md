Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewayv2-vpc-link-errors
Issue-Step: task#4
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewayv2-vpc-link-errors
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigateway|GetVpcLinks|2xx->2xx|$.items[name="sim2real-wire-vpclink-41c8b32f"].vpcLinkStatusMessage
Issue-Commit: fa605e3beb5560d80c09dff54081711248679782

# apigateway GetVpcLinks: AVAILABLE vpcLinkStatusMessage is emulator-authored prose

> _Note: this issue was identified against the code base at commit `fa605e3beb5560d80c09dff54081711248679782`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetVpcLinks

## Description
Cause 1 of 1: the vpcLinkStatusMessage the emulator derives for a VPC link that has reached status AVAILABLE is its own prose rather than AWS's fixed sentence for that status class, so every VPC-link representation the emulator projects diverges on that member.

AWS: GetVpcLinks returns a collection of VpcLink representations. Smithy (com.amazonaws.apigatewayv2#VpcLink): member VpcLinkStatus -> enum com.amazonaws.apigatewayv2#VpcLinkStatus with values PENDING | AVAILABLE | DELETING | FAILED | INACTIVE; member VpcLinkStatusMessage -> StringWithLengthBetween0And1024, jsonName 'vpcLinkStatusMessage', documented only as 'A message summarizing the cause of the status of the VPC link.' The model does not enumerate the per-status strings, so for the exact text this is recorded behavior only: the recording shows AWS answering 200 and populating vpcLinkStatusMessage with 'VPC link is ready to route traffic' once the link's vpcLinkStatus is AVAILABLE. For the AVAILABLE status class that whole sentence is FIXED text: it interpolates no variable part at all — no link name, link id, subnet id, security-group id, account, region or timestamp is drawn from the request or from state — so it is a reproducible requirement, not a per-run value. The REST-API projection com.amazonaws.apigateway#VpcLink carries the same pair as status/statusMessage, so the same per-status strings apply there.

Emulator: the same member is served as 'The VPC link is available for use by an integration.' The HTTP status is 200 as recorded and every other asserted member of the item (name, vpcLinkId, the vpcLinkStatus enum AVAILABLE, subnetIds, securityGroupIds) matches, so the stored state and the status enum are correct and the divergence is confined to the message string the emulator derives from that status.

Why: vpcLinkStatusMessage is the human-readable half of the VPC-link status contract — what SDK users, CLI output, CloudFormation diagnostics and readiness/polling gates surface and match on while waiting for a link to become usable. A client matching AWS's 'VPC link is ready to route traffic' never matches the emulator, so a genuinely AVAILABLE link reads as not-yet-ready, and any diagnostic echoing the message shows wording AWS never emits. Nothing environment-dependent explains the difference, because the text is derived purely from the status.

Fix: the VPC-link response projection/serializer for the apigateway VPC-link resource — the code deriving statusMessage / vpcLinkStatusMessage from the stored VpcLinkStatus. The per-status strings must be AWS's, and must be applied on every path that projects a VPC link (GetVpcLinks, GetVpcLink, CreateVpcLink, UpdateVpcLink), not just the list path.

Already-open issues named for the fixer, none of which the replay records as covering this replay key (the cluster comes back with an EMPTY owned_by, so re-replaying this fixture/step is the only way to confirm a fix): 20261113T194554Z-apigateway-getvpclinks-derives-a-non-aws-vpclinkstatusmessage-for-an.md, 20261113T111007Z-apigateway-getvpclinks-serves-emulator-authored-vpclinkstatusmessage-prose-for.md, 20261112T104410Z-apigateway-vpc-link-available-status-message-is-emulator-authored.md and 20261108T120854Z-apigateway-getvpclinks-reports-an-emulator-authored-vpclinkstatusmessage-for-an.md all discuss the same AVAILABLE-status wording but are not recorded as owning this cluster; 20261109T154749Z-apigatewayv2-cloudformation-ec2-a-vpc-link-created-by.md covers CloudFormation-created VPC-link visibility, not the status message of a successful representation.

Traces: apigatewayv2-vpc-link-errors#task#4 (GetVpcLinks issued after the VPC link reached AVAILABLE).

## Evidence
Replay cluster (verdict mismatch, service=apigateway, operation=GetVpcLinks, expected status 200 -> actual status 200, affected_count 1, stub_path empty, owned_by empty):
  $.items[name="sim2real-wire-vpclink-<generated>"].vpcLinkStatusMessage: value_mismatch
    expected: 'VPC link is ready to route traffic'
    actual:   'The VPC link is available for use by an integration.'

Fixed vs variable parts: for the AVAILABLE status the entire sentence 'VPC link is ready to route traffic' is fixed text with NO variable parts taken from the request or from emulator state. The VPC-link name inside the JSONPath selector (sim2real-wire-vpclink-...) is a per-run generated name and is explicitly NOT part of the requirement — only the message value is.

Everything else asserted in the step matched: HTTP 200, the vpcLinkStatus enum (AVAILABLE), the link id, the link name, subnetIds and securityGroupIds.

## Steps to Reproduce
1. git_clone(), then replay_wire_corpus().
2. Locate fixture apigatewayv2-vpc-link-errors, step task#4 — the GetVpcLinks call made after the VPC link has reached status AVAILABLE.
3. By hand: CreateVpcLink, poll GetVpcLink until vpcLinkStatus is AVAILABLE, then GET the VPC-link collection and read items[].vpcLinkStatusMessage (status/statusMessage on the REST-API projection).
4. Observe the emulator answers 200 with vpcLinkStatusMessage 'The VPC link is available for use by an integration.' where AWS answers 200 with vpcLinkStatusMessage 'VPC link is ready to route traffic'.
