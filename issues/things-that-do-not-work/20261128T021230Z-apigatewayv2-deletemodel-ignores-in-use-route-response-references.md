Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewayv2-websocket-response-model-in-use-and-route-response-delete
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewayv2-websocket-response-model-in-use-and-route-response-delete
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigatewayv2|DeleteModel|4xx->2xx|$
Issue-Key: apigatewayv2|DeleteModel|4xx->2xx|$headers.content-length
Issue-Key: apigatewayv2|DeleteModel|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: apigatewayv2|DeleteModel|4xx->2xx|status
Issue-Key: apigatewayv2|DeleteModel|2xx->4xx|$
Issue-Key: apigatewayv2|DeleteModel|2xx->4xx|status
Issue-Commit: c05681c6a7a1f98eaadd6408209ad7859bc7b6dc

# apigatewayv2 DeleteModel ignores in-use route-response references and destroys the model, so the later delete 404s

> _Note: this issue was identified against the code base at commit `c05681c6a7a1f98eaadd6408209ad7859bc7b6dc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteModel

## Description
2 causes, one root cause: DeleteModel performs no referential check, so it deletes a model that a route response still references and then has nothing left to delete when the legitimate delete arrives.

Cause 1 of 2: DeleteModel on a model still referenced by a route response succeeds instead of conflicting.
AWS: Smithy: DeleteModel http {DELETE /v2/apis/{ApiId}/models/{ModelId}, code 204}, modeled errors NotFoundException(404) and TooManyRequestsException(429) — the conflict status is recorded behavior only. The recording shows status 409 with a JSON body, the x-amzn-errortype header and a content-length header, and a message whose fixed text is "Cannot delete model '<modelName>'. It is currently referenced from route with key '<routeKey>' (route response key '<routeResponseKey>')" — the three variable parts are the model's name, the referencing route's routeKey and that route response's routeResponseKey, all taken from the emulator's own state, not from the request.
Emulator: answers 204 with an EMPTY body, no x-amzn-errortype and no content-length, and DELETES the model while the route response still names it.
Why: this is silent data loss with a dangling reference. A client that relies on the service to protect an in-use model gets no refusal, the route response is left pointing at a model that no longer exists, and the message that would have told it which route holds the reference is never produced — so it cannot even find what to detach.
Fix: DeleteModel's handler in the apigatewayv2 model plane — before removing the model, scan route responses (and route request models) for references and raise the 409 conflict with AWS's templated message; the error serializer must attach x-amzn-errortype and content-length.
Traces: apigatewayv2-websocket-response-model-in-use-and-route-response-delete#task#7.

Cause 2 of 2: after the referencing route response is removed, DeleteModel answers 404 instead of the modeled 204.
AWS: Smithy: DeleteModel http code 204 with an empty output shape (Unit); NotFoundException is modeled at 404 for a model that does not exist. Recorded behavior: once the route response has been deleted the reference is gone, the model is still present, and the delete succeeds 204 with an empty body.
Emulator: answers 404 with body {'__type': 'NotFoundException', 'message': 'Invalid model identifier specified: <modelId>'} — because cause 1 already destroyed the model, there is nothing left to delete.
Why: the detach-then-delete sequence, which is the only correct way to remove an in-use model, fails at its final step; a client cleaning up an API cannot complete teardown and sees a not-found it cannot explain.
Fix: follows from cause 1 (the in-use check must prevent the premature delete); this step is the observable confirmation that the model was destroyed early.
Traces: apigatewayv2-websocket-response-model-in-use-and-route-response-delete#task#11.

## Evidence
Cause 1 — expected status 409 with body {'message': "Cannot delete model '<modelName>'. It is currently referenced from route with key '<routeKey>' (route response key '<routeResponseKey>')"} plus x-amzn-errortype and content-length headers; actual status 204 with an empty body ({}) and neither header. Cause 2 — expected status 204 with an empty body; actual status 404 with {'__type': 'NotFoundException', 'message': 'Invalid model identifier specified: <modelId>'}.

## Steps to Reproduce
1. Replay apigatewayv2-websocket-response-model-in-use-and-route-response-delete step task#7 (DeleteModel while a route response still references the model): expect 409 with the 'Cannot delete model ...' conflict message; the emulator answers 204 and deletes the model.
2. Replay the same fixture step task#11 (DeleteModel after the route response has been deleted): expect 204 with an empty body; the emulator answers 404 NotFoundException because the model was already removed in step 1.
