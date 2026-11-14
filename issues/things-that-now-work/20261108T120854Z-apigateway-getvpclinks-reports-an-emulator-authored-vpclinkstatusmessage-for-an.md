Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewayv2-vpc-link-errors
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewayv2-vpc-link-errors
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigateway|GetVpcLinks|2xx->2xx|$.items[name="sim2real-wire-vpclink-306b18c4"].vpcLinkStatusMessage
Issue-Commit: 386e377a24364e2ae13f63408a712b163326bf5e

# GetVpcLinks reports an emulator-authored vpcLinkStatusMessage for an AVAILABLE VPC link

> _Note: this issue was identified against the code base at commit `386e377a24364e2ae13f63408a712b163326bf5e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetVpcLinks

## Description
Cause 1 of 1: the VPC link summary served by GetVpcLinks carries a status message the emulator wrote itself instead of the sentence AWS attaches to an AVAILABLE VPC link.

AWS: Smithy (com.amazonaws.apigatewayv2) — GetVpcLinks returns VpcLinks whose items are VpcLink structures; VpcLink.VpcLinkStatusMessage (jsonName vpcLinkStatusMessage, target StringWithLengthBetween0And1024) is documented as "A message summarizing the cause of the status of the VPC link", and VpcLink.VpcLinkStatus is the enum PENDING | AVAILABLE | DELETING | FAILED | INACTIVE. The model does not enumerate the message text, so for the wording this is recorded behavior only: the recording shows that once a VPC link has settled into AVAILABLE, AWS reports the fixed sentence "VPC link is ready to route traffic" in vpcLinkStatusMessage, for every VPC link, with no variable parts interpolated from the request or from state (no link name, id, subnet or security-group value appears in it). The emulator's text is the whole string, so there is no request- or state-derived portion of it that could legitimately differ between environments.

Emulator: the same AVAILABLE link is listed with vpcLinkStatusMessage = "The VPC link is available for use by an integration." — a differently worded sentence of the emulator's own composition. Status (200) and every other member of the item match; only this string diverges.

Why: a client that keys off the VPC link's status message — a console or CLI surface that renders it, a polling helper that waits for the readiness sentence, or a test that asserts on it — sees text AWS never emits. The readiness signal reads as an unknown state, so readiness waiters do not terminate and assertions on the AWS sentence fail even though the link really is AVAILABLE.

Fix: the VpcLink representation for the apigatewayv2 VPC link resource — whatever composes vpcLinkStatusMessage when a link settles (the VPC link state/provisioning transition that stamps the message, surfaced through the GetVpcLinks/GetVpcLink serializer). The message for the AVAILABLE status must be AWS's fixed sentence "VPC link is ready to route traffic", not an emulator-authored paraphrase; messages for the other VpcLinkStatus values must likewise be AWS's text rather than authored prose.

Related but NOT covering this: open issue 20261104T121154Z-apigatewayv2-getvpclinks-stub.md ("Implement apigatewayv2 GetVpcLinks: unimplemented scaffold stub") concerns the apigatewayv2-token GetVpcLinks stub; here the apigateway-token GetVpcLinks does answer 200 with a populated item and diverges only on the status message. Open issues 20261103T135737Z-apigateway-createvpclink-s-asynchronous-provisioning-settles-a-link.md and 20261106T131339Z-apigatewayv2-2-net-new-root-causes-createvpclink-s.md concern CreateVpcLink's provisioning verdict and its subnet-not-found refusal envelope, not the AVAILABLE readiness wording returned by GetVpcLinks.

## Evidence
Fixture apigatewayv2-vpc-link-errors, step task#4, GetVpcLinks, status 200 expected and 200 received.

Expected (recorded AWS response): $.items[name="<vpc-link-name>"].vpcLinkStatusMessage == "VPC link is ready to route traffic"
Actual (emulator): $.items[name="<vpc-link-name>"].vpcLinkStatusMessage == "The VPC link is available for use by an integration."

Replay detail verbatim: $.items[name="sim2real-wire-vpclink-306b18c4"].vpcLinkStatusMessage: value_mismatch (expected 'VPC link is ready to route traffic', got 'The VPC link is available for use by an integration.'). The VPC link name in that path is a per-run generated name and is not part of the requirement; the requirement is the message string, which AWS emits as fixed text with no interpolated values.

Smithy consulted: VpcLink.VpcLinkStatusMessage (StringWithLengthBetween0And1024, jsonName vpcLinkStatusMessage); VpcLinkStatus enum values PENDING, AVAILABLE, DELETING, FAILED, INACTIVE.

## Steps to Reproduce
1. Create a VPC link with valid subnet and security-group ids and wait for its vpcLinkStatus to reach AVAILABLE.
2. Call GetVpcLinks (GET /v2/vpclinks) and locate the item for that link.
3. Read items[].vpcLinkStatusMessage: the emulator answers "The VPC link is available for use by an integration." where AWS answers "VPC link is ready to route traffic".

Equivalently: replay fixture apigatewayv2-vpc-link-errors and inspect step task#4.
