Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewayv2-http-api-route-config-cleanup
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewayv2-http-api-route-config-cleanup apigatewayv2-route-request-parameter-lifecycle
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigateway|DeleteRouteRequestParameter|2xx->2xx|$headers.content-length
Issue-Key: apigateway|GetRoute|2xx->2xx|$.requestParameters
Issue-Commit: 11bf1cc5011aa260df7304f2661cde6d9105badf

# apigateway route request-parameter deletion leaves an empty requestParameters map and an unframed 204

> _Note: this issue was identified against the code base at commit `11bf1cc5011aa260df7304f2661cde6d9105badf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteRouteRequestParameter

## Description
Two net-new causes on the HTTP API route request-parameter path: the 204 DELETE is not framed like AWS's, and after the last request parameter is removed the route representation keeps an empty map instead of omitting the member. Both clusters came back with empty stub_path and empty owned_by, so no open issue carries their replay keys.

Cause 1 of 2: DeleteRouteRequestParameter's 204 response omits the Content-Length header.
AWS: Smithy (apigatewayv2) DeleteRouteRequestParameter — http DELETE /v2/apis/{ApiId}/routes/{RouteId}/requestparameters/{RequestParameterKey}, code 204, output shape Unit (no members); errors NotFoundException (404), TooManyRequestsException (429). Recorded behavior: the 204 carries an explicit Content-Length whose value is 0 (sibling 204 DELETE clusters in this corpus record the literal '0'), i.e. an empty-but-framed body. The header name and the value 0 are the fixed requirement; no part of it varies per request.
Emulator: answers 204 with no Content-Length header at all (detail: `$headers.content-length: missing (expected '<shaped-not-asserted>', got None)`).
Why: a client or proxy that reads response framing from Content-Length (HTTP/1.1 keep-alive pools, strict SDK/CLI transports, recording proxies) cannot tell a framed empty body from a response whose body is still to come; it either blocks on the socket or treats the connection as non-reusable, so an otherwise successful delete looks like a transport fault.
Fix: the DeleteRouteRequestParameter handler's response serializer (the shared 204/no-content response builder for apigatewayv2 route child resources) — emit Content-Length: 0 on empty 204 bodies.
Traces: apigatewayv2-http-api-route-config-cleanup#task#6 (also apigatewayv2-route-request-parameter-lifecycle#task#5).

Cause 2 of 2: GetRoute serializes requestParameters as an empty object once every parameter has been deleted.
AWS: Smithy (apigatewayv2) GetRoute — http GET /v2/apis/{ApiId}/routes/{RouteId}, code 200; output member RequestParameters (jsonName 'requestParameters', target RouteParameters, a map of __string -> ParameterConstraints), not required and carrying no default. Recorded behavior: when the route has no request parameters, AWS omits the member entirely rather than returning {}.
Emulator: returns `"requestParameters": {}` (detail: `$.requestParameters: unexpected_present (expected None, got {})`).
Why: an absent map and an empty map are different facts to a client. Code that branches on member presence (`if 'requestParameters' in route`) or that diffs a route against desired state sees a configuration block that AWS never reports, so an idempotent apply keeps trying to reconcile a route that is already clean, and snapshot/diff tests of the route representation never match AWS.
Fix: the route representation serializer/state for apigatewayv2 routes — drop empty maps (requestParameters, and the same rule for requestModels) from the serialized route instead of emitting {} after the last entry is removed.
Traces: apigatewayv2-http-api-route-config-cleanup#task#7 (also apigatewayv2-route-request-parameter-lifecycle#task#6).

## Evidence
Replay of simparity-wire-aws-v1 (emulator up, 908 steps, 59 divergence clusters). Clusters with empty stub_path and empty owned_by:

1) divergence_key `service=apigateway|DeleteRouteRequestParameter|status=204->204|keys=$headers.content-length`, fixture apigatewayv2-http-api-route-config-cleanup step task#6, verdict mismatch, expected_status 204 / actual_status 204, 2 affected steps (apigatewayv2-http-api-route-config-cleanup#task#6, apigatewayv2-route-request-parameter-lifecycle#task#5). Detail: `$headers.content-length: missing (expected '<shaped-not-asserted>', got None)`. Sibling recorded 204 DELETE clusters in the same corpus assert the literal value '0'.

2) divergence_key `service=apigateway|GetRoute|status=200->200|keys=$.requestParameters`, fixture apigatewayv2-http-api-route-config-cleanup step task#7, verdict mismatch, expected_status 200 / actual_status 200, 2 affected steps (apigatewayv2-http-api-route-config-cleanup#task#7, apigatewayv2-route-request-parameter-lifecycle#task#6). Detail: `$.requestParameters: unexpected_present (expected None, got {})`.

Status codes match in both clusters; the divergences are purely the response framing and the serialized route shape.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay simparity-wire-aws-v1.
2. Replay apigatewayv2-http-api-route-config-cleanup#task#6: DELETE /v2/apis/{apiId}/routes/{routeId}/requestparameters/{key}. Expected: 204 with Content-Length: 0. Observed: 204 with no Content-Length header.
3. Replay apigatewayv2-http-api-route-config-cleanup#task#7: GET /v2/apis/{apiId}/routes/{routeId} after the last request parameter was deleted. Expected: body with no requestParameters member. Observed: `"requestParameters": {}`.
4. apigatewayv2-route-request-parameter-lifecycle#task#5 and #task#6 reproduce the same two causes.
