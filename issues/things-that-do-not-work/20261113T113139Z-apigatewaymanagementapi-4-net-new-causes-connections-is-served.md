Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewaymanagementapi.forbidden-without-manage-connections-permission-rejected apigatewaymanagementapi.manage-connections-denial-precedes-connection-lookup apigatewaymanagementapi.agwmgmt-identity-members-and-response-envelopes apigatewaymanagementapi.agwmgmt-post-payload-byte-ceiling-and-unread-socket-refusal apigatewaymanagementapi.post-to-connection-fragments-large-payload-across-frames
Issue-Shape-Id: com.amazonaws.apigatewaymanagementapi#ApiGatewayManagementApi
Issue-Key: apigatewaymanagementapi.forbidden-without-manage-connections-permission-rejected|_error#8|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: apigatewaymanagementapi.manage-connections-denial-precedes-connection-lookup|_assert_refused#16|AssertionError|assert code == want_code, (
Issue-Key: apigatewaymanagementapi.agwmgmt-identity-members-and-response-envelopes|run#25|AssertionError|assert "UserAgent" not in bare_identity, (
Issue-Key: apigatewaymanagementapi.agwmgmt-post-payload-byte-ceiling-and-unread-socket-refusal|run#33|AssertionError|assert refusal["message"] == "Too Many Requests. WebSocket client buffer full", (
Issue-Key: apigatewaymanagementapi.post-to-connection-fragments-large-payload-across-frames|run#22|AssertionError|assert len(frames) >= 2, (
Issue-Commit: 2bc4f9350f4794788afdc3a762d704c686e40d96

# apigatewaymanagementapi: 4 net-new causes — @connections is served with no execute-api:ManageConnections authorization, Identity.UserAgent is synthesised as "", the buffer-full LimitExceededException has no message, and PostToConnection never fragments past the 32 KB frame quota

> _Note: this issue was identified against the code base at commit `2bc4f9350f4794788afdc3a762d704c686e40d96`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PostToConnection

## Description
Four independent root causes in the ApiGatewayManagementApi @connections data plane, all reached now that the WEBSOCKET handshake itself succeeds (most checks in this service pass). None is covered by an open issue: the nearest ones (20261029T214905Z-apigatewaymanagementapi-connections-error-layer-every-failure-answers-410.md and 20261104T221013Z-apigatewaymanagementapi-connections-splits-400-410-on-did-this.md) describe the 400/410 envelope split, not the authorization gate, the Identity member, the 429 message or frame fragmentation.

Cause 1 of 4: @connections requests are served without evaluating execute-api:ManageConnections, so an unprivileged caller is let through and, for an absent id, is answered by the connection lookup instead of a denial.
AWS: the service is sigv4-only (Smithy: ApiGatewayManagementApi authSchemes [sigv4]; common errors ForbiddenException @httpError 403, GoneException @httpError 410). An IAM principal holding no execute-api:ManageConnections grant is refused HTTP 403 with error code AccessDeniedException. The message's fixed text is "User: <caller> is not authorized to perform: execute-api:ManageConnections on resource: <resource-arn> because no identity-based policy allows the execute-api:ManageConnections action"; its variable parts come from the request and the caller's identity — <caller> is the calling session ARN, and <resource-arn> is arn:aws:execute-api:<region>:<account with all but the last four digits masked by '*'>:<apiId>/<stage>/<HTTP method of the operation>/@connections/{connectionId}. The literal route template "@connections/{connectionId}" is fixed text: AWS never interpolates the caller's connection id into it, precisely because authorization is evaluated BEFORE the connection is looked up. That ordering means the same caller gets 403 AccessDeniedException for a live id and for an absent one alike.
Emulator: PostToConnection from an assumed-role session with no inline policy at all is served HTTP 200 with an empty body (`{'ResponseMetadata': {'HTTPStatusCode': 200, ...}}`) — the call that AWS refuses SUCCEEDS. The same unprivileged session calling GET /@connections/<absent id> is answered 'GoneException' with HTTP 410 and an empty Message — the identical answer an AUTHORISED caller gets for that id, i.e. the request reached connection lookup with no authorization decision taken at all.
Why: a client relying on AWS's behavior cannot test or demonstrate least-privilege for @connections. An unpermissioned principal can push arbitrary data to, inspect and disconnect live WebSocket clients, and callers that branch on AccessDeniedException vs GoneException (retry vs prune) take the wrong branch.
Fix: the @connections data-plane request handler's authorization step for ApiGatewayManagementApi — evaluate the sigv4 identity against execute-api:ManageConnections on the api/stage/method resource and serialize AccessDeniedException (403) before any connection lookup; the lookup must run only after the grant is confirmed.
Traces: apigatewaymanagementapi.forbidden-without-manage-connections-permission-rejected, apigatewaymanagementapi.manage-connections-denial-precedes-connection-lookup.

Cause 2 of 4: GetConnection synthesises an empty Identity.UserAgent for a handshake that carried no User-Agent header.
AWS: Smithy GetConnectionResponse.Identity -> Identity{SourceIp (jsonName sourceIp), UserAgent (jsonName userAgent)}, both smithy.api#required with clientOptional. Recorded behavior: when the WebSocket handshake carried NO User-Agent header, AWS OMITS the userAgent member from the identity object entirely rather than synthesising a value, even though the member is modeled required; when the client does send one it is reported verbatim.
Emulator: for a connection whose handshake sent no User-Agent, GetConnection reports Identity = {SourceIp: <peer ip>, UserAgent: ''}. The control connection in the same check, which did send a User-Agent, is reported with that value, so the member is populated from the handshake when present and defaulted to "" when absent.
Why: a client that distinguishes "the service did not observe a user agent" from "the client sent an empty user agent" is told the latter; a membership test on the identity map is always true, so no caller can detect the absent case.
Fix: the GetConnection response serializer plus the connection-record state that captures handshake headers — keep the User-Agent optional and skip the member when the handshake had none.
Traces: apigatewaymanagementapi.agwmgmt-identity-members-and-response-envelopes.

Cause 3 of 4: the buffer-full refusal carries an empty error message.
AWS: Smithy LimitExceededException @httpError 429, documented "The client is sending more than the allowed number of requests per unit of time or the WebSocket client side buffer is full." For a connection whose client is not reading, PostToConnection is refused 429 LimitExceededException with the fixed message "Too Many Requests. WebSocket client buffer full" — all fixed text, no variable parts; it names the buffer rather than a request rate.
Emulator: the 429 LimitExceededException for the unread connection is serialized with message '' (no message member at all).
Why: a client cannot tell a client-side-buffer refusal from genuine request-rate throttling, so back-off-and-retry is applied where the correct action is to stop pushing to or disconnect that socket.
Fix: the PostToConnection handler's buffer-full refusal path and the LimitExceededException serializer — populate the message.
Traces: apigatewaymanagementapi.agwmgmt-post-payload-byte-ceiling-and-unread-socket-refusal.

Cause 4 of 4: PostToConnection delivers an over-quota payload as a single WebSocket frame instead of fragmenting it.
AWS: recorded behavior only (the Smithy model constrains only the payload size via PayloadTooLargeException @httpError 413). The WebSocket frame quota is 32 KB and is not adjustable, so a message larger than that is delivered to the client as a fragmented sequence — a first data frame (opcode 0x1/0x2, FIN 0), zero or more continuation frames (opcode 0x0) and a final frame with FIN 1 — with no single frame payload exceeding the quota. A ~99.5 KB payload therefore arrives as at least 4 frames.
Emulator: a 99538-byte payload arrived as ONE frame carrying all 99538 bytes.
Why: a client built against AWS's framing (one that reassembles continuation frames, sizes its read buffer to the 32 KB quota, or streams fragments) is handed a single oversize frame and either over-allocates, stalls or drops the message.
Fix: the PostToConnection delivery path / the WebSocket frame writer in the apigatewaymanagementapi data plane — split the payload at the frame quota and emit continuation frames.
Traces: apigatewaymanagementapi.post-to-connection-fragments-large-payload-across-frames.

## Evidence
Cause 1:
- expected: PostToConnection by a principal with no execute-api:ManageConnections grant -> HTTP 403, code AccessDeniedException, message "User: <caller> is not authorized to perform: execute-api:ManageConnections on resource: arn:aws:execute-api:<region>:********<last4>:<apiId>/<stage>/POST/@connections/{connectionId} because no identity-based policy allows the execute-api:ManageConnections action".
  actual: HTTP 200, empty body: {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/json', 'content-length': '0', ...}}} — "expected a rejection, but the call SUCCEEDED".
- expected: GET /@connections/<absent id> by the same unprivileged caller -> AccessDeniedException, HTTP 403 (authorization precedes lookup; the message names the route template, never the id).
  actual: 'GoneException', HTTP 410, message '' — identical to what an AUTHORISED caller sees for that id.

Cause 2:
- expected: GetConnection on a connection whose handshake sent no User-Agent -> identity with no userAgent member.
  actual: Identity = {'SourceIp': <peer ip>, 'UserAgent': ''}. Control connection in the same check, which sent 'sim2real-behav-agent/1.0 (measurement)', was reported with that value.

Cause 3:
- expected: 429 LimitExceededException, message "Too Many Requests. WebSocket client buffer full".
  actual: 429 LimitExceededException, message ''.

Cause 4:
- expected: a 99538-byte PostToConnection payload delivered as >= 4 frames, none above the 32 KB frame quota.
  actual: "a 99538-byte payload arrived as ONE frame of 99538 bytes".

## Steps to Reproduce
1. Create a WEBSOCKET apigatewayv2 API with a Lambda-backed $connect/$default/$disconnect, deploy a stage, and open a client socket to it; learn the connection id from the $default echo.
2. Cause 1a: assume a role with NO inline policy, build an apigatewaymanagementapi client on the stage's management endpoint with those credentials and call PostToConnection(ConnectionId=<live id>, Data=b"denied"). Observe HTTP 200 instead of 403 AccessDeniedException.
3. Cause 1b: with the same unprivileged credentials call GetConnection(ConnectionId=<never-minted well-formed id>). Observe 410 GoneException with an empty message instead of 403 AccessDeniedException.
4. Cause 2: open a socket whose handshake sends no User-Agent header, then call GetConnection on it with a permissioned client. Observe Identity.UserAgent == '' instead of the member being absent.
5. Cause 3: open a socket and never read from it, push until the client buffer fills, and inspect the LimitExceededException. Observe an empty message.
6. Cause 4: PostToConnection a ~99.5 KB payload and read raw frames off the client socket. Observe one 99538-byte frame instead of >= 4 fragments.
