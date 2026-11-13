Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewayv2-vpc-link-errors
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewayv2-vpc-link-errors
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigateway|GetVpcLinks|2xx->2xx|$.items[name="sim2real-wire-vpclink-6c7b81d0"].vpcLinkStatusMessage
Issue-Commit: 2bc4f9350f4794788afdc3a762d704c686e40d96

# apigateway GetVpcLinks serves emulator-authored vpcLinkStatusMessage prose for an AVAILABLE VPC link

> _Note: this issue was identified against the code base at commit `2bc4f9350f4794788afdc3a762d704c686e40d96`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetVpcLinks

## Description
Cause 1 of 1: the VPC link status message served on the GetVpcLinks list path is prose the emulator authored, not the sentence AWS attaches to an AVAILABLE VPC link.

AWS: GetVpcLinks returns `items[]`, each a VpcLink whose `vpcLinkStatus` is one of PENDING | AVAILABLE | DELETING | FAILED | INACTIVE and whose `vpcLinkStatusMessage` is, per the model, "A message summarizing the cause of the status of the VPC link" (Smithy: com.amazonaws.apigatewayv2#VpcLink members `vpcLinkStatus` -> VpcLinkStatus enum, `vpcLinkStatusMessage` -> StringWithLengthBetween0And1024). The model fixes no text, so this is recorded behavior only: for a VPC link in status AVAILABLE, AWS serves the fixed sentence `VPC link is ready to route traffic`. That sentence has no variable part — it is fixed text keyed off the AVAILABLE status, and carries no subnet id, security-group id, name or VPC link id from the request or from state.

Emulator: for the same AVAILABLE VPC link the emulator serves `The VPC link is available for use by an integration.` in `items[<the link>].vpcLinkStatusMessage` (HTTP 200, otherwise matching). The status itself matches; only the message diverges, and it diverges as a whole sentence (different words, different punctuation — the emulator's ends in a period), so no part of it is AWS's wording.

Why: the status message is the field a client reads to explain a VPC link's state to an operator or to match on while polling a link toward readiness. A client that asserts, logs or greps AWS's AVAILABLE sentence sees a message it never matches, so a readiness check keyed on the message never fires even though the link is in fact AVAILABLE, and any UI or runbook echoing the field shows text that does not exist in AWS.

Fix: the VPC link status-message derivation that fills VpcLink.vpcLinkStatusMessage when the resource is projected for the read/list path — the message must be derived from the link's status, with AVAILABLE mapping to AWS's fixed sentence, and the same mapping must be used by GetVpcLink/GetVpcLinks/CreateVpcLink/UpdateVpcLink so the single and list paths cannot drift apart.

Related open issues (none of which carries this step's replay key, so this divergence is still unowned and unexercised by their fixes): 20261108T120854Z-apigateway-getvpclinks-reports-an-emulator-authored-vpclinkstatusmessage-for-an.md, 20261108T144540Z-apigateway-getvpclinks-reports-an-emulator-authored-vpclinkstatusmessage-for.md, 20261112T104410Z-apigateway-vpc-link-available-status-message-is-emulator-authored.md, 20261111T014037Z-apigateway-admits-an-unresolvable-restendpointidentifier-and-reworded-vpc.md and 20261113T093743Z-apigateway-derived-response-fields-diverge-from-aws-apiendpoint.md.

Traces: apigatewayv2-vpc-link-errors#task#4 (GetVpcLinks, 200 -> 200, key $.items[...].vpcLinkStatusMessage).

## Evidence
Fixture apigatewayv2-vpc-link-errors, step task#4, operation GetVpcLinks, verdict mismatch, expected status 200 / actual status 200.

Expected (AWS recording): $.items[name="<vpc link name>"].vpcLinkStatusMessage == "VPC link is ready to route traffic"
Actual (emulator):       $.items[name="<vpc link name>"].vpcLinkStatusMessage == "The VPC link is available for use by an integration."

Replay detail verbatim: `$.items[name="sim2real-wire-vpclink-6c7b81d0"].vpcLinkStatusMessage: value_mismatch (expected 'VPC link is ready to route traffic', got 'The VPC link is available for use by an integration.')`. The link name in that detail is the per-run generated name from the fixture and is not part of the requirement; the requirement is the message text for status AVAILABLE, which is fixed. divergence_key: service=apigateway|GetVpcLinks|status=200->200|keys=$.items[name=...].vpcLinkStatusMessage. affected_count: 1.

## Steps to Reproduce
1. Replay wire fixture apigatewayv2-vpc-link-errors against the emulator (trace set simparity-wire-aws-v1).
2. The fixture creates a VPC link and lets it settle to vpcLinkStatus AVAILABLE.
3. At step task#4 call GetVpcLinks and read the entry for the created link.
4. Observe vpcLinkStatus == AVAILABLE but vpcLinkStatusMessage == "The VPC link is available for use by an integration." instead of AWS's "VPC link is ready to route traffic".
