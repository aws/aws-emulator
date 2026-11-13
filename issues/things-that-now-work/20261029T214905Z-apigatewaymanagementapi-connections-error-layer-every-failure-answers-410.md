Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewaymanagementapi-connection-envelope-errors
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewaymanagementapi-connection-envelope-errors apigatewaymanagementapi-connection-stage-authority-and-shape-validation apigatewaymanagementapi-get-connection-malformed-id-shape-variants-and-replay
Issue-Shape-Id: com.amazonaws.apigatewaymanagementapi#ApiGatewayManagementApi
Issue-Key: apigatewaymanagementapi|GetConnection|4xx->4xx|$
Issue-Key: apigatewaymanagementapi|PostToConnection|4xx->4xx|$
Issue-Key: apigatewaymanagementapi|DeleteConnection|4xx->4xx|$
Issue-Key: apigatewaymanagementapi|GetConnection|4xx->4xx|$.message
Issue-Key: apigatewaymanagementapi|GetConnection|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: apigatewaymanagementapi|GetConnection|4xx->4xx|status
Issue-Key: apigatewaymanagementapi|PostToConnection|4xx->4xx|$.message
Issue-Key: apigatewaymanagementapi|PostToConnection|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: apigatewaymanagementapi|PostToConnection|4xx->4xx|status
Issue-Key: apigatewaymanagementapi|DeleteConnection|4xx->4xx|$.message
Issue-Key: apigatewaymanagementapi|DeleteConnection|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: apigatewaymanagementapi|DeleteConnection|4xx->4xx|status
Issue-Commit: 89c27cbcf326b5c1a1abaeca904759cc614d9072

# ApiGatewayManagementApi @connections error layer: every failure answers 410 GoneException with a __type body, never 400 BadRequestException or 403 ForbiddenException

> _Note: this issue was identified against the code base at commit `89c27cbcf326b5c1a1abaeca904759cc614d9072`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetConnection

## Description
NINE net-new wire root causes in the ApiGatewayManagementApi (@connections) data-plane error layer, all on the same emulator service token `apigatewaymanagementapi`. They share one fault: the error layer has exactly ONE outcome for a failing @connections request — a 410 GoneException whose body carries a `__type` member — so (a) the one error it does emit is serialized in the wrong envelope and (b) the two refusals that must precede the connection lookup (ConnectionId shape validation, stage authority) do not exist, and their requests fall through to that same 410. Causes are numbered below; each carries its own fixture and step and ALL NINE must be closed before this issue is submitted.

Common model facts (smithy_query on services/service-com-amazonaws-apigatewaymanagementapi-apigatewaymanagementapi/smithy-model/com-amazonaws-apigatewaymanagementapi-apigatewaymanagementapi.json): sdkId ApiGatewayManagementApi, protocol restJson1, auth sigv4; GetConnection GET /@connections/{ConnectionId}; PostToConnection POST /@connections/{ConnectionId} errors [PayloadTooLargeException]; DeleteConnection DELETE /@connections/{ConnectionId} code 204. Service common errors: ForbiddenException (@httpError 403, NO members, doc "The caller is not authorized to invoke this operation."), GoneException (@httpError 410, NO members, doc "The connection with the provided id no longer exists."), LimitExceededException. ConnectionId is an @httpLabel @required __string with NO pattern and NO length constraint; BadRequestException is NOT in the model, so the 400 refusal below is recorded behavior only.

=== Cause 1 of 9: GetConnection's 410 GoneException body carries a `__type` member; AWS serializes GoneException as an EMPTY JSON object ===
AWS: Smithy GoneException is a client error, @httpError 410, with NO members — restJson1 therefore has nothing to serialize into the body. Recorded: a well-formed but unknown ConnectionId on GetConnection answers HTTP 410 with the body `{}` exactly (no `__type`, no `message`); the error's identity travels in the x-amzn-errortype response header, whose value the fixture only shape-asserts. The status (410) and error code (GoneException) are requirements; the body is the fixed literal `{}` with no variable parts.
Emulator: same status 410, but the body is `{"__type": "GoneException"}`. Replay detail: `$: value_mismatch (expected {}, got {'__type': 'GoneException'})`.
Why: a client that deserializes the 410 body against the modelled (memberless) GoneException shape sees an unmodelled member; strict SDKs and hand-rolled parsers that reject unknown keys, and any code that round-trips or hashes the error body, behave differently against the emulator than against AWS. It also teaches callers to read the error code out of the body, which AWS does not populate for this error.
Fix: the restJson1 error serializer for the @connections operations — emit memberless modelled errors as a bare `{}` document and carry the error identity only in x-amzn-errortype, instead of injecting a synthetic `__type` discriminator.
Traces: apigatewaymanagementapi-connection-envelope-errors#task#6 (cluster of 5 steps, also in apigatewaymanagementapi-connection-stage-authority-and-shape-validation#task#6/#task#12 and apigatewaymanagementapi-get-connection-malformed-id-shape-variants-and-replay#task#6/#task#9).

=== Cause 2 of 9: PostToConnection's 410 GoneException body carries the same `__type` member ===
AWS: as Cause 1 — GoneException is memberless, @httpError 410; the recorded PostToConnection response to an unknown but well-formed ConnectionId is HTTP 410 with body `{}` and the error code in x-amzn-errortype.
Emulator: HTTP 410 with body `{"__type": "GoneException"}`. Replay detail: `$: value_mismatch (expected {}, got {'__type': 'GoneException'})`.
Why: same as Cause 1, on the send path — the operation an application calls on every server push, so the malformed envelope is on the hottest error path.
Fix: the same restJson1 error serializer, applied to PostToConnection's error responses.
Traces: apigatewaymanagementapi-connection-envelope-errors#task#7.

=== Cause 3 of 9: DeleteConnection's 410 GoneException body carries the same `__type` member ===
AWS: as Cause 1; DeleteConnection's success is a 204 with no body, and its GoneException is HTTP 410 with body `{}`.
Emulator: HTTP 410 with body `{"__type": "GoneException"}`. Replay detail: `$: value_mismatch (expected {}, got {'__type': 'GoneException'})`.
Why: same as Cause 1, on the teardown path.
Fix: the same restJson1 error serializer, applied to DeleteConnection's error responses.
Traces: apigatewaymanagementapi-connection-envelope-errors#task#8.

=== Cause 4 of 9: GetConnection does not validate the ConnectionId path label's shape — a malformed id reaches the connection store and is answered 410 GoneException instead of 400 BadRequestException ===
AWS: the model puts NO pattern or length on ConnectionId and does not declare BadRequestException, so this refusal is recorded behavior only. Recorded: a ConnectionId that is not the shape API Gateway mints is refused BEFORE the connection store is consulted, with HTTP 400, error code BadRequestException and the message `Invalid connectionId: <ConnectionId>` — the fixed text is `Invalid connectionId: ` and the variable part is the ConnectionId exactly as the request sent it (the recording's `x` is one such request value, not part of the template). A well-formed but unknown id instead reaches the store and is answered 410 GoneException.
Emulator: HTTP 410 with x-amzn-errortype `GoneException` and no `message` member at all. Replay detail: `status: expected 400, got 410` / `$.message: missing (expected 'Invalid connectionId: x', got None)` / `$headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'GoneException')`.
Why: a client cannot distinguish "I built the id wrong" (400, retrying is pointless, fix the caller) from "the peer has gone away" (410, the documented signal to drop that connection from your own table). An application that prunes its connection table on GoneException silently deletes LIVE connections whenever it passes a corrupted id, and a caller-side id-construction bug is invisible on this emulator.
Fix: shared @connections request validation in front of the three handlers — validate the ConnectionId @httpLabel's shape and return the 400 BadRequestException envelope (with the `message` member) before any connection lookup.
Traces: apigatewaymanagementapi-connection-envelope-errors#task#9 (cluster of 3 steps, also apigatewaymanagementapi-get-connection-malformed-id-shape-variants-and-replay#task#7/#task#8).

=== Cause 5 of 9: PostToConnection does not validate the ConnectionId path label's shape — malformed id answered 410 GoneException instead of 400 BadRequestException ===
AWS: as Cause 4 — recorded behavior only; the malformed-id refusal is HTTP 400, code BadRequestException, message `Invalid connectionId: <ConnectionId>` (fixed text `Invalid connectionId: ` plus the request's ConnectionId), raised before the connection store is consulted.
Emulator: HTTP 410, x-amzn-errortype `GoneException`, no `message`. Replay detail: `status: expected 400, got 410` / `$.message: missing (expected 'Invalid connectionId: x', got None)` / `$headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'GoneException')`.
Why: as Cause 4, and worse on this operation: a push loop that treats 410 as "connection closed" will drop subscribers in response to its own id bug.
Fix: the same shared @connections ConnectionId validation, ordered ahead of PostToConnection's connection lookup.
Traces: apigatewaymanagementapi-connection-stage-authority-and-shape-validation#task#10.

=== Cause 6 of 9: DeleteConnection does not validate the ConnectionId path label's shape — malformed id answered 410 GoneException instead of 400 BadRequestException ===
AWS: as Cause 4; the refusal is HTTP 400 BadRequestException with message `Invalid connectionId: <ConnectionId>` before any lookup.
Emulator: HTTP 410, x-amzn-errortype `GoneException`, no `message`. Replay detail: `status: expected 400, got 410` / `$.message: missing (expected 'Invalid connectionId: x', got None)` / `$headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'GoneException')`.
Why: as Cause 4; a disconnect path that reads 410 as "already gone, nothing to do" treats a malformed-id bug as success.
Fix: the same shared @connections ConnectionId validation, ordered ahead of DeleteConnection's connection lookup.
Traces: apigatewaymanagementapi-connection-stage-authority-and-shape-validation#task#11.

=== Cause 7 of 9: the management endpoint's STAGE segment is ignored on GetConnection — an undeployed stage reaches the connection store and is answered 410 GoneException instead of 403 ForbiddenException ===
AWS: the @connections endpoint is always the per-API https://<api-id>.execute-api.<region>.amazonaws.com/<stage>, so the stage is part of the request path; stage routing is the execute-api data plane and is not a modelled operation, hence recorded behavior — but the model carries the error AWS uses: ForbiddenException, @httpError 403, errorKind client, no members. Recorded: addressing a stage name that is not a deployed stage of that API answers HTTP 403 with error code ForbiddenException and the message `Forbidden` (fixed literal text, no variable parts), WITHOUT consulting the connection store, while the same request against the same API's deployed stage answers 410 GoneException for an unknown id.
Emulator: HTTP 410, x-amzn-errortype `GoneException`, no `message`. Replay detail: `status: expected 403, got 410` / `$.message: missing (expected 'Forbidden', got None)` / `$headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'GoneException')`.
Why: the endpoint's stage is the only thing scoping a @connections call to a deployment. Ignoring it makes the emulator over-permissive: a client pointed at a stage that was never deployed (a typo, a stale or deleted stage) is told the connection is gone rather than that it may not invoke that endpoint, so misconfiguration is indistinguishable from a closed socket and a per-stage authorization boundary that exists on AWS does not exist here.
Fix: the @connections request router / endpoint resolution — resolve {api-id, stage} from the request path against the API's deployed stages and return the 403 ForbiddenException envelope (with message `Forbidden`) before dispatching to the connection store.
Traces: apigatewaymanagementapi-connection-stage-authority-and-shape-validation#task#7.

=== Cause 8 of 9: the STAGE segment is ignored on PostToConnection — undeployed stage answered 410 GoneException instead of 403 ForbiddenException ===
AWS: as Cause 7 — an undeployed stage in the endpoint path answers HTTP 403 ForbiddenException with the fixed message `Forbidden`, before the connection store is consulted.
Emulator: HTTP 410, x-amzn-errortype `GoneException`, no `message`. Replay detail: `status: expected 403, got 410` / `$.message: missing (expected 'Forbidden', got None)` / `$headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'GoneException')`.
Why: as Cause 7; a producer writing to the wrong stage is told its subscribers vanished instead of that the endpoint is not authorized, and will prune a healthy connection table.
Fix: the same @connections endpoint/stage resolution, applied before PostToConnection dispatch.
Traces: apigatewaymanagementapi-connection-stage-authority-and-shape-validation#task#8.

=== Cause 9 of 9: the STAGE segment is ignored on DeleteConnection — undeployed stage answered 410 GoneException instead of 403 ForbiddenException ===
AWS: as Cause 7 — HTTP 403 ForbiddenException, message `Forbidden`, raised on stage resolution and not on the connection store.
Emulator: HTTP 410, x-amzn-errortype `GoneException`, no `message`. Replay detail: `status: expected 403, got 410` / `$.message: missing (expected 'Forbidden', got None)` / `$headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'GoneException')`.
Why: as Cause 7; a teardown against a mistyped stage reports success-equivalent "already gone" rather than an authorization failure, so the real connection is never closed.
Fix: the same @connections endpoint/stage resolution, applied before DeleteConnection dispatch.
Traces: apigatewaymanagementapi-connection-stage-authority-and-shape-validation#task#9.

Related (does NOT cover these causes): the open behavioral issue 20261029T192401Z-apigatewaymanagementapi-connections-malformed-connection-ids-oversize-payloads-and.md describes the malformed-id and undeployed-stage faults from the simparity-behavioral-aws-v1 suite and carries only that suite's check ids; it does not carry any of the wire fixture/step replay keys listed here (the replay reports no owner for all nine clusters), and it says nothing about the `__type` member in the GoneException body (Causes 1-3). The open issue 20261023T223918Z-apigatewaymanagementapi-is-untestable-end-to-end-the-websocket.md is about the WebSocket upgrade answering 404 and is unrelated to these error envelopes.

## Evidence
Corpus: simparity-wire-aws-v1. Replay summary: emulator_up true, bootstrap_signal false, 3 fixtures replayed, 36 steps, 21 passed, 15 mismatched, 0 blocked/errored, score 58.3%, 9 divergence clusters — ALL nine are apigatewaymanagementapi and none carries a stub_path or an owned_by.

Cause 1 (GetConnection) — fixture apigatewaymanagementapi-connection-envelope-errors, step task#6; divergence_key service=apigatewaymanagementapi|GetConnection|status=410->410|keys=$; expected 410 / actual 410.
  Expected body: {}
  Actual body:   {"__type": "GoneException"}
  detail: $: value_mismatch (expected {}, got {'__type': 'GoneException'})
  affected_count 5: apigatewaymanagementapi-connection-envelope-errors#task#6, apigatewaymanagementapi-connection-stage-authority-and-shape-validation#task#6, apigatewaymanagementapi-connection-stage-authority-and-shape-validation#task#12, apigatewaymanagementapi-get-connection-malformed-id-shape-variants-and-replay#task#6, apigatewaymanagementapi-get-connection-malformed-id-shape-variants-and-replay#task#9

Cause 2 (PostToConnection) — apigatewaymanagementapi-connection-envelope-errors#task#7; expected 410 / actual 410; detail: $: value_mismatch (expected {}, got {'__type': 'GoneException'}).

Cause 3 (DeleteConnection) — apigatewaymanagementapi-connection-envelope-errors#task#8; expected 410 / actual 410; detail: $: value_mismatch (expected {}, got {'__type': 'GoneException'}).

Cause 4 (GetConnection, malformed id) — apigatewaymanagementapi-connection-envelope-errors#task#9; expected 400 / actual 410; detail: status: expected 400, got 410 | $.message: missing (expected 'Invalid connectionId: x', got None) | $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'GoneException'). affected_count 3, also apigatewaymanagementapi-get-connection-malformed-id-shape-variants-and-replay#task#7 and #task#8. Message template: fixed text 'Invalid connectionId: ' + the request's ConnectionId ('x' is the recording's request value).

Cause 5 (PostToConnection, malformed id) — apigatewaymanagementapi-connection-stage-authority-and-shape-validation#task#10; expected 400 / actual 410; same three-part detail (missing $.message 'Invalid connectionId: x', errortype GoneException).

Cause 6 (DeleteConnection, malformed id) — apigatewaymanagementapi-connection-stage-authority-and-shape-validation#task#11; expected 400 / actual 410; same three-part detail.

Cause 7 (GetConnection, undeployed stage) — apigatewaymanagementapi-connection-stage-authority-and-shape-validation#task#7; expected 403 / actual 410; detail: status: expected 403, got 410 | $.message: missing (expected 'Forbidden', got None) | $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'GoneException'). 'Forbidden' is fixed text with no variable parts.

Cause 8 (PostToConnection, undeployed stage) — apigatewaymanagementapi-connection-stage-authority-and-shape-validation#task#8; expected 403 / actual 410; same three-part detail.

Cause 9 (DeleteConnection, undeployed stage) — apigatewaymanagementapi-connection-stage-authority-and-shape-validation#task#9; expected 403 / actual 410; same three-part detail.

Model facts used (smithy_query, model_path services/service-com-amazonaws-apigatewaymanagementapi-apigatewaymanagementapi/smithy-model/com-amazonaws-apigatewaymanagementapi-apigatewaymanagementapi.json):
- digest: sdk_id ApiGatewayManagementApi, protocols [restJson1], authSchemes [sigv4], 3 operations; GetConnection GET /@connections/{ConnectionId}; PostToConnection POST /@connections/{ConnectionId} errors [PayloadTooLargeException]; DeleteConnection DELETE /@connections/{ConnectionId} code 204; common_errors [ForbiddenException, GoneException, LimitExceededException] — no BadRequestException.
- shape GoneException: client error, httpError 410, members {} (none), doc 'The connection with the provided id no longer exists.' -> nothing to serialize in the body.
- shape ForbiddenException: client error, httpError 403, members {} (none), doc 'The caller is not authorized to invoke this operation.'
- shape GetConnectionRequest: ConnectionId @httpLabel @required __string, no pattern, no length.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from that checkout, then replay the simparity-wire-aws-v1 corpus (replay_wire_corpus). The three apigatewaymanagementapi fixtures are apigatewaymanagementapi-connection-envelope-errors, apigatewaymanagementapi-connection-stage-authority-and-shape-validation and apigatewaymanagementapi-get-connection-malformed-id-shape-variants-and-replay.
2. Setup each fixture performs: create an apigatewayv2 API with ProtocolType=WEBSOCKET, a MOCK integration, a $connect route, a deployment and a stage; build an apigatewaymanagementapi (restJson1, sigv4) client whose endpoint_url is that API id + deployed stage, with retries disabled.
3. Causes 1-3 (GoneException envelope): call GetConnection, PostToConnection(Data=b'..') and DeleteConnection with a WELL-FORMED but unknown ConnectionId against the deployed stage. Each answers HTTP 410 as expected, but inspect the raw response body: it is {"__type": "GoneException"} where AWS returns exactly {} and conveys the error only in x-amzn-errortype. Fixture/steps: apigatewaymanagementapi-connection-envelope-errors#task#6, #task#7, #task#8.
4. Causes 4-6 (ConnectionId shape validation): repeat the same three calls with a MALFORMED ConnectionId (e.g. a single character, characters outside the base64 alphabet, unpadded base64, or an overlong id). Observe HTTP 410 with x-amzn-errortype GoneException and no message member, instead of HTTP 400 with x-amzn-errortype BadRequestException and message 'Invalid connectionId: <the ConnectionId sent>'. Fixture/steps: apigatewaymanagementapi-connection-envelope-errors#task#9, apigatewaymanagementapi-connection-stage-authority-and-shape-validation#task#10 and #task#11 (plus apigatewaymanagementapi-get-connection-malformed-id-shape-variants-and-replay#task#7/#task#8).
5. Causes 7-9 (stage authority): build a second management client for the SAME ApiId but a stage name that was never deployed, and call GetConnection / PostToConnection / DeleteConnection with the well-formed unknown ConnectionId. Observe HTTP 410 GoneException on all three instead of HTTP 403 with x-amzn-errortype ForbiddenException and message 'Forbidden'; the same calls against the deployed stage still answer 410. Fixture/steps: apigatewaymanagementapi-connection-stage-authority-and-shape-validation#task#7, #task#8, #task#9.
6. After fixing, re-replay the three fixtures and confirm all 36 steps pass: the 410 bodies must be {}, malformed ids must answer 400 BadRequestException 'Invalid connectionId: <id>', and undeployed-stage requests must answer 403 ForbiddenException 'Forbidden'.
