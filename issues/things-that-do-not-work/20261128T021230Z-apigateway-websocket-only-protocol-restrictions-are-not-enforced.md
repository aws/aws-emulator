Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewayv2-create-route-members-round-trip-through-get-route
Issue-Step: task#7
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewayv2-create-route-members-round-trip-through-get-route apigatewayv2-route-response-members-round-trip-through-get-route-response
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigateway|CreateRoute|4xx->2xx|$.message
Issue-Key: apigateway|CreateRoute|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: apigateway|CreateRoute|4xx->2xx|status
Issue-Key: apigateway|CreateRouteResponse|4xx->2xx|$.message
Issue-Key: apigateway|CreateRouteResponse|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: apigateway|CreateRouteResponse|4xx->2xx|status
Issue-Key: apigateway|GetRouteResponses|2xx->2xx|$.items[*]
Issue-Commit: c05681c6a7a1f98eaadd6408209ad7859bc7b6dc

# apigateway WebSocket-only protocol restrictions are not enforced on CreateRoute and CreateRouteResponse

> _Note: this issue was identified against the code base at commit `c05681c6a7a1f98eaadd6408209ad7859bc7b6dc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRoute

## Description
3 causes. The emulator accepts two route-plane requests AWS refuses with BadRequestException, so invalid configuration is persisted and then surfaces on reads.

Cause 1 of 3: CreateRoute setting an authorization type on a WebSocket route other than $connect.
AWS: Smithy: CreateRoute.AuthorizationType -> enum AuthorizationType [NONE, AWS_IAM, CUSTOM, JWT]; CreateRoute http {POST /v2/apis/{ApiId}/routes, code 201}; modeled BadRequestException httpError 400. Recorded behavior: on a WEBSOCKET API, a route whose routeKey is not '$connect' and which requests a non-NONE authorizationType is refused 400 with message fixed text in full — 'Currently, authorization is restricted to the $connect route only'. No variable parts. The refusal also carries the x-amzn-errortype header.
Emulator: answers 201 and creates the route; no message and no x-amzn-errortype header are emitted.
Why: a client gets a route it believes is authorization-protected, but on AWS that route cannot exist; the configuration silently diverges and the emulator cannot be used to validate a WebSocket authorization layout before deploying it for real.
Fix: CreateRoute's validation in the apigatewayv2 operation handler — for a WEBSOCKET API, reject a non-NONE AuthorizationType on any routeKey other than $connect with BadRequestException and AWS's fixed sentence.
Traces: apigatewayv2-create-route-members-round-trip-through-get-route#task#7.

Cause 2 of 3: CreateRouteResponse carrying responseParameters on a WebSocket API.
AWS: Smithy: CreateRouteResponse.ResponseParameters -> RouteParameters map; http {POST /v2/apis/{ApiId}/routes/{RouteId}/routeresponses, code 201}; modeled BadRequestException httpError 400. Recorded behavior: parameter mappings are an HTTP-API-only feature, so the request is refused 400 with message fixed text in full — 'Parameter mappings are only applicable to HTTP-protocol APIs'. No variable parts. The refusal carries x-amzn-errortype.
Emulator: answers 201 and stores the route response including responseParameters.
Why: a WebSocket route response with parameter mappings is accepted here and rejected on AWS, so an IaC template or SDK script validated against the emulator fails on the real service.
Fix: CreateRouteResponse's validation in the apigatewayv2 operation handler — refuse ResponseParameters when the owning API's protocolType is WEBSOCKET.
Traces: apigatewayv2-route-response-members-round-trip-through-get-route-response#task#4.

Cause 3 of 3: GetRouteResponses lists the route response cause 2 should never have created.
AWS: Smithy: GetRouteResponses http {GET /v2/apis/{ApiId}/routes/{RouteId}/routeresponses}; its items collection holds only route responses that exist. Recorded behavior: the collection has no entry, because the create in cause 2 was refused.
Emulator: returns 200 with an extra items[0] carrying modelSelectionExpression, responseModels, responseParameters, routeResponseId and routeResponseKey '$default'.
Why: the list read reports a resource that does not exist on AWS, so reconciliation loops and drift detection see phantom state.
Fix: follows from cause 2 (CreateRouteResponse validation); this step is the observable confirmation on the read path.
Traces: apigatewayv2-route-response-members-round-trip-through-get-route-response#task#5.

## Evidence
Cause 1 — expected status 400 with $.message='Currently, authorization is restricted to the $connect route only' and an x-amzn-errortype header; actual status 201 with no message and no x-amzn-errortype. Cause 2 — expected status 400 with $.message='Parameter mappings are only applicable to HTTP-protocol APIs' and x-amzn-errortype; actual 201. Cause 3 — expected items[0] absent; actual items[0]={'modelSelectionExpression': '$request.body.kind', 'responseModels': {...}, 'responseParameters': {...}, 'routeResponseId': <id>, 'routeResponseKey': '$default'}.

## Steps to Reproduce
1. Replay apigatewayv2-create-route-members-round-trip-through-get-route step task#7 (CreateRoute with a non-NONE authorizationType on a non-$connect WebSocket route): expect 400 'Currently, authorization is restricted to the $connect route only'; the emulator answers 201.
2. Replay apigatewayv2-route-response-members-round-trip-through-get-route-response step task#4 (CreateRouteResponse with responseParameters on a WebSocket API): expect 400 'Parameter mappings are only applicable to HTTP-protocol APIs'; the emulator answers 201.
3. Replay the same fixture step task#5 (GetRouteResponses): the emulator lists the route response created in step 2.
