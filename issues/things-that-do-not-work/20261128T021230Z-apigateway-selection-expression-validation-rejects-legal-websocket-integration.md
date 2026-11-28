Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewayv2-integration-response-members-round-trip-through-get-integration-response
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewayv2-integration-response-members-round-trip-through-get-integration-response apigatewayv2-route-response-members-round-trip-through-get-route-response
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigateway|CreateIntegrationResponse|2xx->4xx|$.contentHandlingStrategy
Issue-Key: apigateway|CreateIntegrationResponse|2xx->4xx|$.integrationResponseId
Issue-Key: apigateway|CreateIntegrationResponse|2xx->4xx|$.integrationResponseKey
Issue-Key: apigateway|CreateIntegrationResponse|2xx->4xx|$.responseTemplates
Issue-Key: apigateway|CreateIntegrationResponse|2xx->4xx|$.templateSelectionExpression
Issue-Key: apigateway|CreateIntegrationResponse|2xx->4xx|status
Issue-Key: apigateway|CreateIntegrationResponse|4xx->4xx|$.message
Issue-Key: apigateway|GetIntegrationResponses|2xx->2xx|$.items[*]
Issue-Key: apigateway|CreateRouteResponse|2xx->4xx|$.modelSelectionExpression
Issue-Key: apigateway|CreateRouteResponse|2xx->4xx|$.responseModels
Issue-Key: apigateway|CreateRouteResponse|2xx->4xx|$.routeResponseId
Issue-Key: apigateway|CreateRouteResponse|2xx->4xx|$.routeResponseKey
Issue-Key: apigateway|CreateRouteResponse|2xx->4xx|status
Issue-Key: apigateway|GetRouteResponses|2xx->2xx|$.items[*].routeResponseId
Issue-Commit: c05681c6a7a1f98eaadd6408209ad7859bc7b6dc

# apigateway selection-expression validation rejects legal WebSocket integration-response and route-response members

> _Note: this issue was identified against the code base at commit `c05681c6a7a1f98eaadd6408209ad7859bc7b6dc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateIntegrationResponse

## Description
5 causes. The emulator's selection-expression checker rejects the documented WebSocket selection expressions, so integration responses and route responses that AWS creates cannot be created here; the sub-resource is absent and every read of it fails too. The same checker also fires on the wrong member, producing the wrong refusal message when a genuinely invalid request arrives.

Cause 1 of 5: CreateIntegrationResponse with templateSelectionExpression '$integration.response.statuscode' is refused 400 instead of created.
AWS: Smithy: CreateIntegrationResponse.TemplateSelectionExpression -> SelectionExpression (an unconstrained string; the shape doc points at the WebSocket selection-expressions guide); http {POST /v2/apis/{ApiId}/integrations/{IntegrationId}/integrationresponses, code 201}. Recorded behavior: the request is accepted 201 and returns integrationResponseId, integrationResponseKey, responseTemplates, templateSelectionExpression and contentHandlingStrategy 'CONVERT_TO_TEXT'.
Emulator: answers 400 and returns none of those members; the integration response is never stored. Its refusal sentence is 'Unexpected variable in selection expression: $integration', i.e. it treats the documented $integration root as an unknown variable.
Why: the WebSocket integration-response template selection expression is the standard way to pick a response template per status code; no client can configure it against this emulator.
Fix: the selection-expression validator used by CreateIntegrationResponse in the apigatewayv2 operation handler — $integration (and its .response.* path) must be an accepted root for TemplateSelectionExpression.
Traces: apigatewayv2-integration-response-members-round-trip-through-get-integration-response#task#5.

Cause 2 of 5: an invalid responseParameters mapping is refused with the selection-expression message instead of AWS's mapping-expression message.
AWS: Smithy: CreateIntegrationResponse.ResponseParameters -> IntegrationParameters map; modeled BadRequestException httpError 400. Recorded behavior: an unusable response-parameter mapping key is refused 400 with the mapping-validation message whose fixed text is 'Invalid mapping expression specified: Validation Result: warnings : [], errors : [Invalid mapping expression specified: <mapping>]' — the only variable part is <mapping>, the offending mapping expression taken verbatim from the request's responseParameters (in the recording, 'method.response.header.x-source').
Emulator: answers 400 but with 'Unexpected variable in selection expression: $integration' — it refuses for the templateSelectionExpression of cause 1 and never evaluates the responseParameters at all, so the message names the wrong member and omits the offending mapping.
Why: the caller is told the wrong member is at fault and is given no way to identify which mapping expression was rejected, because AWS's message is the only place the offending mapping is echoed.
Fix: CreateIntegrationResponse's validation ordering plus the response-parameter mapping validator in the apigatewayv2 operation handler — validate mappings and report them in AWS's Validation Result form.
Traces: apigatewayv2-integration-response-members-round-trip-through-get-integration-response#task#3.

Cause 3 of 5: GetIntegrationResponses returns an empty collection because cause 1 stored nothing.
AWS: Smithy: GetIntegrationResponses http {GET /v2/apis/{ApiId}/integrations/{IntegrationId}/integrationresponses}. Recorded behavior: items[0] is the integration response created earlier, carrying integrationResponseId, integrationResponseKey, responseTemplates, templateSelectionExpression and contentHandlingStrategy 'CONVERT_TO_TEXT'.
Emulator: items[0] is absent.
Why: the read-back leg of every WebSocket integration-response lifecycle reports nothing configured.
Fix: follows from cause 1; this step is the observable confirmation on the read path.
Traces: apigatewayv2-integration-response-members-round-trip-through-get-integration-response#task#7.

Cause 4 of 5: CreateRouteResponse with modelSelectionExpression '$request.body.kind' is refused 400 instead of created.
AWS: Smithy: CreateRouteResponse.ModelSelectionExpression -> SelectionExpression (unconstrained string); CreateRouteResponse.ResponseModels -> RouteModels map; http code 201. Recorded behavior: accepted 201, returning routeResponseId, routeResponseKey '$default', responseModels and modelSelectionExpression.
Emulator: answers 400; none of routeResponseId/routeResponseKey/responseModels/modelSelectionExpression is returned and the route response is not stored.
Why: a WebSocket route response that selects its response model from the request body cannot be configured at all, so model-validated WebSocket replies cannot be modelled on the emulator.
Fix: the selection-expression validator used by CreateRouteResponse in the apigatewayv2 operation handler — $request.body.* must be an accepted ModelSelectionExpression.
Traces: apigatewayv2-route-response-members-round-trip-through-get-route-response#task#6.

Cause 5 of 5: GetRouteResponses reports a routeResponseId other than the one the create returned.
AWS: Smithy: GetRouteResponses items[].routeResponseId -> Id. Recorded behavior: the listed routeResponseId is the id returned by the preceding CreateRouteResponse (the fixture binds it as a capture, so the requirement is the identity relation, not the literal id).
Emulator: lists an id that does not match the create it should correspond to, because the create in cause 4 was refused and the listed entry is the leftover one from the request AWS rejected (see the companion issue on unenforced WebSocket protocol restrictions).
Why: a client cannot correlate the id it was handed with the id the list reports, so update/delete by id targets the wrong resource or none.
Fix: follows from cause 4 and from CreateRouteResponse's protocol validation; the route-response state must list exactly the ids it minted.
Traces: apigatewayv2-route-response-members-round-trip-through-get-route-response#task#8.

## Evidence
Cause 1 — expected status 201 with $.integrationResponseId, $.integrationResponseKey, $.responseTemplates, $.templateSelectionExpression='$integration.response.statuscode', $.contentHandlingStrategy='CONVERT_TO_TEXT'; actual 400 with all members missing. Cause 2 — $.message expected 'Invalid mapping expression specified: Validation Result: warnings : [], errors : [Invalid mapping expression specified: method.response.header.x-source]', got 'Unexpected variable in selection expression: $integration' (status 400 both sides). Cause 3 — expected items[0] = the created integration response; actual items[0] missing (got None). Cause 4 — expected status 201 with $.routeResponseId, $.routeResponseKey='$default', $.responseModels, $.modelSelectionExpression='$request.body.kind'; actual 400 with all members missing. Cause 5 — expected items[0].routeResponseId to equal the captured create id; actual a different id.

## Steps to Reproduce
1. Replay apigatewayv2-integration-response-members-round-trip-through-get-integration-response step task#5 (CreateIntegrationResponse with templateSelectionExpression '$integration.response.statuscode'): expect 201; the emulator answers 400 'Unexpected variable in selection expression: $integration'.
2. Replay the same fixture step task#3 (CreateIntegrationResponse with an invalid responseParameters mapping) and compare $.message.
3. Replay the same fixture step task#7 (GetIntegrationResponses): items[0] is absent.
4. Replay apigatewayv2-route-response-members-round-trip-through-get-route-response step task#6 (CreateRouteResponse with modelSelectionExpression '$request.body.kind'): expect 201; the emulator answers 400.
5. Replay the same fixture step task#8 (GetRouteResponses) and compare items[0].routeResponseId with the id the create returned.
