Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewayv2-product-rest-endpoint-page-lifecycle
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewayv2-product-rest-endpoint-page-lifecycle apigatewayv2-vpc-link-errors
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigateway|CreateProductRestEndpointPage|2xx->4xx|$.displayContent
Issue-Key: apigateway|CreateProductRestEndpointPage|2xx->4xx|$.lastModified
Issue-Key: apigateway|CreateProductRestEndpointPage|2xx->4xx|$.productRestEndpointPageArn
Issue-Key: apigateway|CreateProductRestEndpointPage|2xx->4xx|$.productRestEndpointPageId
Issue-Key: apigateway|CreateProductRestEndpointPage|2xx->4xx|$.restEndpointIdentifier
Issue-Key: apigateway|CreateProductRestEndpointPage|2xx->4xx|$.status
Issue-Key: apigateway|CreateProductRestEndpointPage|2xx->4xx|$.tryItState
Issue-Key: apigateway|CreateProductRestEndpointPage|2xx->4xx|status
Issue-Key: apigateway|GetVpcLinks|2xx->2xx|$.items[name="sim2real-wire-vpclink-512f150a"].vpcLinkStatusMessage
Issue-Commit: 11bf1cc5011aa260df7304f2661cde6d9105badf

# apigateway refuses a valid product REST endpoint page and rewords the VPC link status text

> _Note: this issue was identified against the code base at commit `11bf1cc5011aa260df7304f2661cde6d9105badf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateProductRestEndpointPage

## Description
Two net-new causes, both reported with empty stub_path and empty owned_by: one over-strict refusal that breaks the portal-product lifecycle, and one reworded status string.

Cause 1 of 2: CreateProductRestEndpointPage refuses a restEndpointIdentifier that DOES resolve, answering 400 instead of creating the page.
AWS: Smithy (apigatewayv2) CreateProductRestEndpointPage — http POST /v2/portalproducts/{PortalProductId}/productrestendpointpages, code 201; required input members PortalProductId (@httpLabel) and RestEndpointIdentifier (jsonName 'restEndpointIdentifier'), whose IdentifierParts requires method, path, restApiId and stage; errors BadRequestException (400), NotFoundException (404), AccessDeniedException (403), TooManyRequestsException (429). On success AWS answers 201 with productRestEndpointPageId (10..30 chars, ^[a-z0-9]+$), productRestEndpointPageArn (20..2048), restEndpointIdentifier echoed back with its identifierParts, displayContent.endpoint, status (Status enum: AVAILABLE | IN_PROGRESS | FAILED — recorded IN_PROGRESS for a fresh page), tryItState (TryItState enum — recorded ENABLED) and lastModified. The recorded request names an API id and a stage that exist in the same fixture, so 400 is not a legal answer here.
Emulator: answers 400 and creates nothing — none of the modeled success members is present (detail lists $.productRestEndpointPageId, $.productRestEndpointPageArn, $.restEndpointIdentifier, $.displayContent, $.status, $.tryItState, $.lastModified all missing). Hypothesis: the identifier-resolution check added for the unresolvable-identifier case looks the restApiId/stage pair up in the wrong registry (e.g. only among v1 REST APIs/stages) and therefore rejects an API and stage that the emulator itself created, so a valid create is refused.
Why: this is the create step of the product-REST-endpoint-page lifecycle. A client that legitimately publishes a product page for one of its own APIs is told its request is invalid, so the page never exists and every later read, update, try-it and delete on it is unreachable — the whole portal-product publishing flow is blocked, not merely cosmetically wrong.
Fix: the CreateProductRestEndpointPage handler's input validation — resolve restEndpointIdentifier.identifierParts (restApiId + stage, then method/path) against the state where those APIs and stages actually live, admit the page and return 201 with the modeled members; keep BadRequestException (400) for identifiers that genuinely do not resolve.
Traces: apigatewayv2-product-rest-endpoint-page-lifecycle#task#6.

Cause 2 of 2: GetVpcLinks reports emulator-authored prose as an AVAILABLE VPC link's status message.
AWS: Smithy (apigateway) VpcLink.statusMessage, documented 'A description about the VPC link status'; the equivalent apigatewayv2 VpcLink.VpcLinkStatusMessage (jsonName 'vpcLinkStatusMessage', StringWithLengthBetween0And1024) is documented 'A message summarizing the cause of the status of the VPC link'. Recorded behavior: for a link whose status is AVAILABLE, the message is the fixed text 'VPC link is ready to route traffic' — entirely fixed, with no variable parts taken from the request or state.
Emulator: returns 'The VPC link is available for use by an integration.'
Why: the status message is the human- and script-readable explanation of the link's state; a client, a wait/poll loop or a snapshot test that matches AWS's text finds no match and cannot distinguish a ready link from an unexpected state.
Fix: the VpcLink state/serializer — the per-status status-message table shared by GetVpcLinks/GetVpcLink/CreateVpcLink.
Traces: apigatewayv2-vpc-link-errors#task#4. This replay key came back unowned because the existing open issues key the cluster by a per-run generated link name; the same wording defect is described in 20261111T014037Z-apigateway-admits-an-unresolvable-restendpointidentifier-and-reworded-vpc.md (Cause 2), 20261112T104410Z-apigateway-vpc-link-available-status-message-is-emulator-authored.md, 20261113T093743Z-apigateway-derived-response-fields-diverge-from-aws-apiendpoint.md and 20261113T111007Z-apigateway-getvpclinks-serves-emulator-authored-vpclinkstatusmessage-prose-for.md, so a fixer should treat it as one shared root cause with those.

## Evidence
Replay of simparity-wire-aws-v1. Clusters with empty stub_path and empty owned_by:

1) divergence_key `service=apigateway|CreateProductRestEndpointPage|status=201->400|keys=$.displayContent,$.lastModified,$.productRestEndpointPageArn,$.productRestEndpointPageId,$.restEndpointIdentifier,$.status,$.tryItState,status`, fixture apigatewayv2-product-rest-endpoint-page-lifecycle step task#6, verdict mismatch, expected_status 201 / actual_status 400, 1 affected step. Detail: `status: expected 201, got 400`; `$.productRestEndpointPageId: missing`; `$.productRestEndpointPageArn: missing (expected 'arn:aws:apigateway:us-west-2:000000000000:/portalproducts/<portalProductId>/productrestendpointpages/<pageId>')`; `$.restEndpointIdentifier: missing (expected identifierParts {method: 'GET', path: '/items', restApiId: <apiId>, stage: <stageName>})`; `$.status: missing (expected 'IN_PROGRESS')`; `$.tryItState: missing (expected 'ENABLED')`; `$.displayContent: missing`; `$.lastModified: missing`. The api id, stage name, arn and page id in the recording are per-run values, not requirements; the requirement is the 201 and the modeled member set.

2) divergence_key `service=apigateway|GetVpcLinks|status=200->200|keys=$.items[name="..."].vpcLinkStatusMessage`, fixture apigatewayv2-vpc-link-errors step task#4, verdict mismatch, 200/200, 1 affected step. Detail: `vpcLinkStatusMessage: value_mismatch (expected 'VPC link is ready to route traffic', got 'The VPC link is available for use by an integration.')`. The link name in the selector is a per-run generated value.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay simparity-wire-aws-v1.
2. Replay apigatewayv2-product-rest-endpoint-page-lifecycle#task#6: POST /v2/portalproducts/{portalProductId}/productrestendpointpages with a restEndpointIdentifier.identifierParts naming the API id and stage created earlier in the same fixture (method GET, path /items). Expected: 201 with productRestEndpointPageId, productRestEndpointPageArn, restEndpointIdentifier, displayContent, status IN_PROGRESS, tryItState ENABLED, lastModified. Observed: 400 with none of those members, and no page created.
3. Replay apigatewayv2-vpc-link-errors#task#4: create a VPC link, let it reach status AVAILABLE, then GET /vpclinks and read the entry's vpcLinkStatusMessage. Expected the fixed text 'VPC link is ready to route traffic'; observed 'The VPC link is available for use by an integration.'
