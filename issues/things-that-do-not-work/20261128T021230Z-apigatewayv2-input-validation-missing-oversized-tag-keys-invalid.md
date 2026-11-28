Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewayv2-api-tag-key-value-boundaries
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewayv2-api-tag-key-value-boundaries apigatewayv2-jwt-querystring-authorizer-read-back-and-delete apigatewayv2-websocket-mock-integration-members-and-timeout-ceiling
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigatewayv2|TagResource|4xx->2xx|$
Issue-Key: apigatewayv2|TagResource|4xx->2xx|$headers.content-length
Issue-Key: apigatewayv2|TagResource|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: apigatewayv2|TagResource|4xx->2xx|status
Issue-Key: apigatewayv2|CreateAuthorizer|4xx->2xx|$.message
Issue-Key: apigatewayv2|CreateAuthorizer|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: apigatewayv2|CreateAuthorizer|4xx->2xx|status
Issue-Key: apigatewayv2|GetAuthorizers|2xx->2xx|$.items[*]
Issue-Key: apigatewayv2|CreateIntegration|4xx->2xx|$.message
Issue-Key: apigatewayv2|CreateIntegration|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: apigatewayv2|CreateIntegration|4xx->2xx|status
Issue-Commit: c05681c6a7a1f98eaadd6408209ad7859bc7b6dc

# apigatewayv2 input validation missing: oversized tag keys, invalid JWT identity sources and out-of-range timeouts are accepted

> _Note: this issue was identified against the code base at commit `c05681c6a7a1f98eaadd6408209ad7859bc7b6dc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
5 causes. Three apigatewayv2 write operations accept requests AWS refuses with a 400, and two further reads then expose the resource that should never have been created.

Cause 1 of 5: TagResource accepts a tag key longer than AWS's key-length bound.
AWS: Smithy: TagResource.Tags -> Tags, a map whose key is __string and whose value is StringWithLengthBetween1And1600 (so the key bound is not model-stated); modeled BadRequestException httpError 400. Recorded behavior: a tag key outside 1..128 characters is refused 400 with a JSON body, the x-amzn-errortype header and a content-length header. The message's fixed text is "1 validation error detected: Value '<offending-map>' at 'addedTags' failed to satisfy constraint: Map keys must satisfy constraint: [Member must have length less than or equal to 128, Member must have length greater than or equal to 1]" — the only variable part is <offending-map>, the rejected entry rendered as {key=value} from the request's own tags.
Emulator: answers 204 with an EMPTY body, no x-amzn-errortype header and no content-length header; the oversized key is written.
Why: a client that relies on the service to bound tag keys stores a tag the real service rejects, and because the emulator returns success it never learns which key was at fault.
Fix: TagResource's input validation in the apigatewayv2 tagging handler — enforce the 1..128 key-length bound and raise BadRequestException with AWS's templated sentence (and the error headers the error serializer attaches).
Traces: apigatewayv2-api-tag-key-value-boundaries#task#4, apigatewayv2-api-tag-key-value-boundaries#task#5, apigatewayv2-api-tag-key-value-boundaries#task#6.

Cause 2 of 5: CreateAuthorizer accepts a JWT authorizer with multiple/invalid identity sources.
AWS: Smithy: CreateAuthorizer.IdentitySource -> IdentitySourceList (required), AuthorizerType -> enum [REQUEST, JWT]; http {POST /v2/apis/{ApiId}/authorizers, code 201}; modeled BadRequestException httpError 400. Recorded behavior: the request is refused 400 with x-amzn-errortype and a message whose fixed text is 'Invalid identity source expression: <sources>. The source must be a request header or request querystring.' — the variable part <sources> is the request's identitySource entries joined with commas (in the recording, '$request.querystring.token,$request.header.Authorization').
Emulator: answers 201 and creates the authorizer; no message and no x-amzn-errortype are emitted.
Why: the emulator lets a JWT authorizer be configured in a way AWS rejects, so an API that validates here is refused at deploy time on the real service, and the identity source that will actually be consulted is undefined.
Fix: CreateAuthorizer's identity-source validation in the apigatewayv2 operation handler — check the identitySource list against the JWT authorizer rule and raise BadRequestException with AWS's templated sentence.
Traces: apigatewayv2-jwt-querystring-authorizer-read-back-and-delete#task#3.

Cause 3 of 5: GetAuthorizers lists the authorizer cause 2 should never have created.
AWS: Smithy: GetAuthorizers http {GET /v2/apis/{ApiId}/authorizers}. Recorded behavior: items has no entry for the refused authorizer, so items[1] does not exist.
Emulator: returns 200 with an extra items[1] carrying authorizerId, authorizerType JWT, identitySource, jwtConfiguration and name.
Why: the list read reports a resource AWS does not have, so drift detection and reconciliation see phantom state.
Fix: follows from cause 2; this step is the observable confirmation on the read path.
Traces: apigatewayv2-jwt-querystring-authorizer-read-back-and-delete#task#6.

Cause 4 of 5: GetAuthorizers still lists the phantom authorizer after the legitimate one is deleted.
AWS: Smithy: GetAuthorizers as above. Recorded behavior: after the lifecycle's delete the collection is empty, so items[0] does not exist.
Emulator: returns 200 with items[0] still carrying the phantom JWT authorizer.
Why: the teardown leg cannot reach an empty-collection state, so an idempotent cleanup loop never converges.
Fix: follows from cause 2; this step is the observable confirmation after delete.
Traces: apigatewayv2-jwt-querystring-authorizer-read-back-and-delete#task#10.

Cause 5 of 5: CreateIntegration accepts a timeoutInMillis above the WebSocket ceiling.
AWS: Smithy: CreateIntegration.TimeoutInMillis -> IntegerWithLengthBetween50And30000 with @range min 50 max 30000; modeled BadRequestException httpError 400. Recorded behavior: on a WebSocket API the effective ceiling is lower than the model's 30000, and a request above it is refused 400 with x-amzn-errortype and the message whose fixed text is 'Timeout should be between 50 ms and 29000 ms' — the two bounds are the protocol's limits, not values from the request.
Emulator: answers 201 and creates the integration; no message and no x-amzn-errortype are emitted.
Why: a WebSocket integration is created with a timeout AWS will not accept, so the configuration is unportable and the emulator cannot be used to find the ceiling.
Fix: CreateIntegration's TimeoutInMillis validation in the apigatewayv2 operation handler — apply the protocol-specific ceiling (not just the model @range) and raise BadRequestException with AWS's fixed sentence.
Traces: apigatewayv2-websocket-mock-integration-members-and-timeout-ceiling#task#2.

## Evidence
Cause 1 — expected status 400 with a JSON message body, x-amzn-errortype and content-length; actual status 204 with an empty body and neither header (3 affected steps). Cause 2 — expected status 400 with $.message='Invalid identity source expression: $request.querystring.token,$request.header.Authorization. The source must be a request header or request querystring.' and x-amzn-errortype; actual 201. Cause 3 — expected items[1] absent; actual items[1]={authorizerId, authorizerType 'JWT', identitySource [...], jwtConfiguration {...}, name}. Cause 4 — expected items[0] absent; actual items[0]= the same phantom authorizer. Cause 5 — expected status 400 with $.message='Timeout should be between 50 ms and 29000 ms' and x-amzn-errortype; actual 201.

## Steps to Reproduce
1. Replay apigatewayv2-api-tag-key-value-boundaries step task#4 (TagResource with a tag key longer than 128 characters): expect 400 with the addedTags map-key constraint message; the emulator answers 204 empty.
2. Replay apigatewayv2-jwt-querystring-authorizer-read-back-and-delete step task#3 (CreateAuthorizer, JWT, with two identity sources): expect 400; the emulator answers 201.
3. Replay the same fixture steps task#6 and task#10 (GetAuthorizers): the phantom authorizer is listed before and after the delete.
4. Replay apigatewayv2-websocket-mock-integration-members-and-timeout-ceiling step task#2 (CreateIntegration with timeoutInMillis above the WebSocket ceiling): expect 400 'Timeout should be between 50 ms and 29000 ms'; the emulator answers 201.
