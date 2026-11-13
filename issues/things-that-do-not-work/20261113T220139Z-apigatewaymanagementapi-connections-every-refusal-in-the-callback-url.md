Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewaymanagementapi-wsbe-traj-callback-url-forms
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewaymanagementapi-wsbe-traj-callback-url-forms
Issue-Shape-Id: com.amazonaws.apigatewaymanagementapi#ApiGatewayManagementApi
Issue-Key: apigatewaymanagementapi|PostToConnection|4xx->4xx|$
Issue-Key: apigatewaymanagementapi|PostToConnection|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: apigatewaymanagementapi|PostToConnection|4xx->4xx|status
Issue-Key: apigatewaymanagementapi|GetConnection|4xx->4xx|$
Issue-Key: apigatewaymanagementapi|GetConnection|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: apigatewaymanagementapi|GetConnection|4xx->4xx|status
Issue-Key: apigatewaymanagementapi|DeleteConnection|4xx->4xx|$
Issue-Key: apigatewaymanagementapi|DeleteConnection|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: apigatewaymanagementapi|DeleteConnection|4xx->4xx|status
Issue-Commit: fa605e3beb5560d80c09dff54081711248679782

# apigatewaymanagementapi @connections: every refusal in the callback-URL-forms trajectory answers 410 GoneException with an empty body instead of 400 BadRequestException or 403 ForbiddenException (4 causes)

> _Note: this issue was identified against the code base at commit `fa605e3beb5560d80c09dff54081711248679782`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PostToConnection

## Description
FOUR net-new causes on the apigatewaymanagementapi service token, all from the wire fixture `apigatewaymanagementapi-wsbe-traj-callback-url-forms` (simparity-wire-aws-v1). The replay marks none of them as blocked by a stub and none as owned by an open issue, so all four must be closed before this issue is submitted. They share ONE fault: the @connections data plane has exactly one refusal it can reach for these requests — the registry miss, HTTP 410 GoneException with an empty body — so both refusals that must happen BEFORE the registry is consulted (the 400 BadRequestException for a ConnectionId that is not a connection identifier, and the 403 ForbiddenException for a callback URL that addresses no deployed stage) are never produced.

=== Cause 1 of 4: PostToConnection with a ConnectionId AWS refuses as invalid is answered 410 GoneException with an empty body instead of 400 BadRequestException "Invalid connectionId: <ConnectionId>" (fixture apigatewaymanagementapi-wsbe-traj-callback-url-forms, step task#6) ===
AWS: the refusal is recorded behavior only — the Smithy model (ApiGatewayManagementApi, restJson1/sigv4) declares NO BadRequestException shape at all; PostToConnection is POST /@connections/{ConnectionId} with ConnectionId an @httpLabel @required __string carrying NO @pattern and NO @length (smithy_query shape=PostToConnectionRequest), and the service's common errors are only ForbiddenException (@httpError 403, no members, doc "The caller is not authorized to invoke this operation."), GoneException (@httpError 410, no members, doc "The connection with the provided id no longer exists.") and LimitExceededException (429). Recorded response for this step: HTTP 400 with body {"message": "Invalid connectionId: <ConnectionId>"} — the fixed text is "Invalid connectionId: " and the variable part is the ConnectionId exactly as the request sent it (in this recording the id the trajectory submits). The refusal is decided from the submitted VALUE and happens before the connection registry is read, which is why it is 400 and not 410.
Emulator: HTTP 410 with the error type header x-amzn-errortype: GoneException and an EMPTY JSON body ({}) — no `message` member at all. Replay detail: "status: expected 400, got 410 / $: empty_body (expected {'message': 'Invalid connectionId: ...'}, got {}) / $headers.x-amzn-errortype: ... got 'GoneException'". The reason is visible in the checkout: the validity test is `is_well_formed_connection_id` = "standard base64, padded, decoding to exactly 11 bytes", so an id that satisfies that byte-length rule but is still not an identifier AWS accepts falls straight through the 400 gate into the registry, misses there, and is answered the registry-miss 410. The emulator's notion of "could never have been minted" is therefore strictly narrower than AWS's.
Why: 410 GoneException is the documented "this peer went away" signal and the branch a fan-out application prunes its own connection table on; 400 BadRequestException means "your request is malformed, retrying or pruning is pointless". A client handed 410 for an id AWS calls invalid deletes live subscriber rows over a client-side id bug on the emulator, and in production gets the opposite answer with no message to explain it. The missing `message` member also means a client that logs the refusal has nothing to log.
Fix: the shared @connections ConnectionId validation in front of the three handlers (the `is_well_formed_connection_id` / `invalid_connection_id` pair in this service's state module and the gate each handler runs before resolving the connection). Widen the validity rule so every ConnectionId form AWS refuses is refused with the 400 BadRequestException envelope — status 400, error code BadRequestException, body {"message": "Invalid connectionId: " + the request's ConnectionId verbatim} — from the value alone, before the registry is consulted; leave the registry miss as the only producer of 410.
Traces: apigatewaymanagementapi-wsbe-traj-callback-url-forms#task#6.

=== Cause 2 of 4: GetConnection with the same invalid ConnectionId is answered 410 GoneException with an empty body instead of 400 BadRequestException (same fixture, step task#7) ===
AWS: GetConnection is GET /@connections/{ConnectionId} (code 200, output GetConnectionResponse{ConnectedAt, LastActiveAt, Identity}); same unconstrained @httpLabel ConnectionId, same three common errors, no BadRequestException in the model — so again recorded behavior only. Recorded response: HTTP 400, body {"message": "Invalid connectionId: <ConnectionId>"}, with the same fixed-text/variable split as Cause 1. The refusal is a property of the submitted id, so it is identical on all three operations.
Emulator: HTTP 410, x-amzn-errortype: GoneException, empty body ({}). services/service-com-amazonaws-apigatewaymanagementapi-apigatewaymanagementapi/src/ops/get_connection.rs tests `is_well_formed_connection_id` first and returns the modelled GoneException whenever the registry has no entry, so an id that passes the narrow base64/11-byte rule can only ever come back 410.
Why: same inversion as Cause 1, on the operation a client uses to probe whether a connection is usable before writing to it — a caller cannot distinguish "I built the id wrong" from "the client disconnected".
Fix: the same shared ConnectionId validation gate, reached from GetConnection's handler.
Traces: apigatewaymanagementapi-wsbe-traj-callback-url-forms#task#7.

=== Cause 3 of 4: a PostToConnection addressed through a callback-URL form AWS refuses is answered 410 GoneException with an empty body instead of 403 ForbiddenException "Forbidden" (same fixture, step task#8) ===
AWS: endpoint/stage authorization on the execute-api data plane is not a modelled operation, so the behavior is recorded; the error it uses IS modelled — ForbiddenException, @httpError 403, errorKind client, no members. Recorded response for this step: HTTP 403 with body {"message": "Forbidden"} — fixed text with NO variable parts (the refusal names neither the api nor the stage), and it is produced WITHOUT consulting the connection registry, so the same id on a properly addressed, deployed stage would instead reach the registry. The trajectory walks several forms of the @connections callback URL; the form at this step is one AWS will not serve, and refusing it is the per-stage authorization boundary.
Emulator: HTTP 410, x-amzn-errortype: GoneException, empty body ({}) — the request was dispatched to the registry instead of being refused in front of it. Replay detail: "status: expected 403, got 410 / $: empty_body (expected {'message': 'Forbidden'}, got {})". The gate exists but almost never fires: src/service/connections_endpoint.rs `addresses_nothing` returns false (i.e. "serve it") whenever the invoke-path prefix in front of /@connections/ is EMPTY, whenever no endpoint authority has been installed into the shared cell, and whenever the installed apigatewayv2 reading answers anything other than EndpointAuthority::Undeployed — so a callback-URL form the gate does not recognise as an undeployed stage is served as if it were the api's own deployed invoke endpoint. Hypothesis: the form this step uses is one of those (no stage segment in the path, or a host the authority reading does not resolve to an api), which is why it is dispatched rather than refused.
Why: the callback URL's host and stage are the only things that scope an @connections call to a deployment. Answering 410 makes the emulator over-permissive AND wrong: a client pointed at a URL form AWS rejects is told its subscribers are gone instead of that it may not invoke that endpoint, so a misconfigured callback URL is indistinguishable from a closed socket, prune logic fires on the emulator and 403s fire in production.
Fix: the @connections endpoint resolution in front of dispatch (src/service/connections_endpoint.rs `addresses_nothing` and its use in src/service.rs, plus the authority reading apigatewayv2 installs). Decide from the request's addressing — host and invoke path, including the case where the path carries no stage segment — whether the endpoint serves a deployed stage of some api, and refuse everything else with 403 ForbiddenException and the body {"message": "Forbidden"} before any handler runs.
Traces: apigatewaymanagementapi-wsbe-traj-callback-url-forms#task#8.

=== Cause 4 of 4: DeleteConnection with the invalid ConnectionId is answered 410 GoneException with an empty body instead of 400 BadRequestException (same fixture, step task#10) ===
AWS: DeleteConnection is DELETE /@connections/{ConnectionId} with success code 204 and the same three common errors and the same unconstrained @httpLabel ConnectionId; the invalid-id refusal is recorded behavior: HTTP 400, body {"message": "Invalid connectionId: <ConnectionId>"} (fixed prefix + the request's ConnectionId verbatim), decided before the registry is touched and before anything is removed.
Emulator: HTTP 410, x-amzn-errortype: GoneException, empty body ({}) — the narrow well-formedness rule passes the id through to the registry, which misses.
Why: on the delete path the inverted answer is actively misleading in both directions — a caller cleaning up its connection table reads 410 as "already gone, drop the row" when AWS is telling it the id it computed is not an identifier at all, so a client-side id defect is laundered into successful-looking cleanup.
Fix: the same shared ConnectionId validation gate, reached from DeleteConnection's handler.
Traces: apigatewaymanagementapi-wsbe-traj-callback-url-forms#task#10.

Related open issues that do NOT cover these four wire steps (none of them carries these replay keys, and the replay reports no owner): 20261104T221013Z-apigatewaymanagementapi-connections-splits-400-410-on-did-this.md describes the OPPOSITE, now-stale symptom (a per-process issued-id set made every id a 400); 20261029T192401Z-apigatewaymanagementapi-connections-malformed-connection-ids-oversize-payloads-and.md and 20261113T113139Z-apigatewaymanagementapi-4-net-new-causes-connections-is-served.md are behavioral-suite issues on other check ids. The wire-corpus issue 20261029T214905Z-apigatewaymanagementapi-connections-error-layer-every-failure-answers-410.md is already resolved and therefore covers nothing here, although the symptom it named has reappeared for these ids.

## Evidence
Corpus: simparity-wire-aws-v1. Summary: emulator_up true, 47 steps replayed, 43 passed, 4 mismatched, score 91.5%, 4 divergence clusters — all four on service apigatewaymanagementapi, all in fixture apigatewaymanagementapi-wsbe-traj-callback-url-forms, none with a stub_path and none with an owned_by.

Cause 1 — fixture apigatewaymanagementapi-wsbe-traj-callback-url-forms, step task#6, operation PostToConnection, verdict mismatch
  divergence_key: service=apigatewaymanagementapi|PostToConnection|status=400->410|keys=$,$headers.x-amzn-errortype,status
  expected: HTTP 400, body {'message': 'Invalid connectionId: <the request ConnectionId>'}
  actual:   HTTP 410, body {} (empty_body), header x-amzn-errortype: GoneException
  detail: "status: expected 400, got 410 / $: empty_body (expected {'message': 'Invalid connectionId: AAAAAAAAAAAAAAA='}, got {}) / $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'GoneException')"

Cause 2 — step task#7, operation GetConnection, verdict mismatch
  divergence_key: service=apigatewaymanagementapi|GetConnection|status=400->410|keys=$,$headers.x-amzn-errortype,status
  expected: HTTP 400, body {'message': 'Invalid connectionId: <the request ConnectionId>'}
  actual:   HTTP 410, body {}, x-amzn-errortype: GoneException

Cause 3 — step task#8, operation PostToConnection, verdict mismatch
  divergence_key: service=apigatewaymanagementapi|PostToConnection|status=403->410|keys=$,$headers.x-amzn-errortype,status
  expected: HTTP 403, body {'message': 'Forbidden'} (fixed text, no variable parts)
  actual:   HTTP 410, body {}, x-amzn-errortype: GoneException

Cause 4 — step task#10, operation DeleteConnection, verdict mismatch
  divergence_key: service=apigatewaymanagementapi|DeleteConnection|status=400->410|keys=$,$headers.x-amzn-errortype,status
  expected: HTTP 400, body {'message': 'Invalid connectionId: <the request ConnectionId>'}
  actual:   HTTP 410, body {}, x-amzn-errortype: GoneException

(The recorded ConnectionId value and the recorded callback-URL host/stage are this run's values; the requirement is the status, the error code and the message template — fixed text plus the request's own ConnectionId — not those literals.)

Model facts used (smithy_query, model_path services/service-com-amazonaws-apigatewaymanagementapi-apigatewaymanagementapi/smithy-model/com-amazonaws-apigatewaymanagementapi-apigatewaymanagementapi.json):
- digest: sdkId ApiGatewayManagementApi, protocols [restJson1], authSchemes [sigv4], 3 operations; GetConnection GET /@connections/{ConnectionId}; PostToConnection POST /@connections/{ConnectionId} errors [PayloadTooLargeException]; DeleteConnection DELETE /@connections/{ConnectionId} code 204; common_errors [ForbiddenException, GoneException, LimitExceededException] — there is NO BadRequestException shape in this model.
- shape GoneException: errorKind client, httpError 410, members {}, doc "The connection with the provided id no longer exists."
- shape ForbiddenException: errorKind client, httpError 403, members {}, doc "The caller is not authorized to invoke this operation."
- shape GetConnectionRequest: ConnectionId -> __string, httpLabel, required, no pattern/length.
- shape PostToConnectionRequest: ConnectionId as above; Data @httpPayload blob @length {min 0, max 131072}.

Code paths in the checkout (for orientation; the fix is behavioral, not these literals):
- services/service-com-amazonaws-apigatewaymanagementapi-apigatewaymanagementapi/src/state.rs lines 87-104: CONNECTION_ID_BYTES = 11 and is_well_formed_connection_id = "standard base64, padded, decodes to exactly 11 bytes" — the whole of the 400 gate; lines 333-356: invalid_connection_id_message -> format!("Invalid connectionId: {connection_id}") and invalid_connection_id -> Scaffolded::bad_request.
- .../src/ops/get_connection.rs lines 148-173: the well-formedness gate runs first, then connection_at(); a miss returns the modelled GoneException with no members, which renders as HTTP 410 with an empty JSON body.
- .../src/service/connections_endpoint.rs lines 61-94: addresses_nothing returns false for an empty invoke-path prefix, for a missing endpoint-authority cell, and for any authority reading other than EndpointAuthority::Undeployed; forbidden() already renders the correct 403 {"message":"Forbidden"} envelope, so the gap is which addressings reach it.

## Steps to Reproduce
1. Clone the repo, build and boot the emulator, then replay the wire fixture apigatewaymanagementapi-wsbe-traj-callback-url-forms from simparity-wire-aws-v1 (steps task#6, task#7, task#8, task#10 are the diverging ones).
2. Cause 1: replay step task#6 — PostToConnection on the trajectory's callback URL with the ConnectionId the recording submits. Observe HTTP 410 with x-amzn-errortype: GoneException and an empty body {} instead of HTTP 400 with {"message": "Invalid connectionId: <that same ConnectionId>"}.
3. Cause 2: replay step task#7 — GetConnection with the same ConnectionId. Observe the same 410/GoneException/{} instead of 400 BadRequestException with the Invalid-connectionId message.
4. Cause 3: replay step task#8 — PostToConnection addressed through the callback-URL form this step uses. Observe HTTP 410 GoneException with an empty body instead of HTTP 403 with {"message": "Forbidden"}; note that the request reached the connection registry at all, i.e. the endpoint/stage gate in src/service/connections_endpoint.rs did not fire for this addressing.
5. Cause 4: replay step task#10 — DeleteConnection with the invalid ConnectionId. Observe HTTP 410 GoneException with an empty body instead of HTTP 400 with the Invalid-connectionId message.
6. After fixing, re-replay all four steps named in Issue-Trace/Issue-Step and trace_ids: each of PostToConnection/GetConnection/DeleteConnection must answer HTTP 400, error code BadRequestException, body {"message": "Invalid connectionId: " + the request's ConnectionId} for a ConnectionId AWS refuses — decided before the registry is read — and the step-task#8 callback-URL form must answer HTTP 403 ForbiddenException with the body {"message": "Forbidden"} before dispatch; a well-formed, properly addressed but unknown id must still answer 410 GoneException with an empty body, so the 43 currently passing steps stay passing.
