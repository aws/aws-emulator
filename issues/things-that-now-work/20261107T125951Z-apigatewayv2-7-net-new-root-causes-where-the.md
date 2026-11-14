Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2.client-initiated-close-invokes-disconnect-route apigatewayv2.websocket-inbound-binary-frame-rejected apigatewayv2.websocket-inbound-message-32kb-boundary apigatewayv2.websocket-broadcast-to-multiple-connections apigatewayv2.websocket-connect-request-authorizer-token-gate apigatewayv2.websocket-route-selection-by-action apigatewayv2.deep-apigatewayv2-3-stagesem-manual-stage-serves-deployment-snapshot apigatewayv2.deep-apigatewayv2-3-stagesem-stage-variable-target-without-invoke-permission-rejected apigatewayv2.delete-stage-stops-serving-requests apigatewayv2.http-api-cors-configuration-overrides-backend-cors-headers apigatewayv2.http-api-cors-preflight-origin-allowlist-and-delete apigatewayv2.e2e-custom-domain-mapping-invoke apigatewayv2.deploy-apigatewayv2-3-cdk-http-api-stripped-lambda-permission-invoke-rejected apigatewayv2.reset-authorizers-cache-server-side apigatewayv2.route-throttling-returns-429-while-twin-route-serves
Issue-Key: apigatewayv2.client-initiated-close-invokes-disconnect-route|_do_ws_handshake#3|ConnectionError|raise ConnectionError(f"websocket handshake failed: {status_line!r}")
Issue-Key: apigatewayv2.websocket-inbound-binary-frame-rejected|_open_ws_connection#2|ConnectionError|raise ConnectionError(f"websocket handshake failed: {status_line!r}")
Issue-Key: apigatewayv2.websocket-inbound-message-32kb-boundary|_open_ws_connection#2|ConnectionError|raise ConnectionError(f"websocket handshake failed: {status_line!r}")
Issue-Key: apigatewayv2.websocket-broadcast-to-multiple-connections|_poll_until#20|Timeout|time.sleep(interval_s)
Issue-Key: apigatewayv2.websocket-connect-request-authorizer-token-gate|_poll_until#22|Timeout|time.sleep(interval_s)
Issue-Key: apigatewayv2.websocket-route-selection-by-action|_poll_until#19|Timeout|time.sleep(interval_s)
Issue-Key: apigatewayv2.deep-apigatewayv2-3-stagesem-manual-stage-serves-deployment-snapshot|run#44|Timeout|assert not wrong, (
Issue-Key: apigatewayv2.deep-apigatewayv2-3-stagesem-stage-variable-target-without-invoke-permission-rejected|_http#27|Timeout|with urllib.request.urlopen(req, timeout=timeout_s) as resp:
Issue-Key: apigatewayv2.delete-stage-stops-serving-requests|_await#13|Timeout|time.sleep(3)
Issue-Key: apigatewayv2.http-api-cors-configuration-overrides-backend-cors-headers|_poll_http#10|Timeout|time.sleep(3)
Issue-Key: apigatewayv2.http-api-cors-preflight-origin-allowlist-and-delete|_poll_http#13|Timeout|time.sleep(3)
Issue-Key: apigatewayv2.e2e-custom-domain-mapping-invoke|_poll_domain_invoke#18|Timeout|time.sleep(interval_s)
Issue-Key: apigatewayv2.deploy-apigatewayv2-3-cdk-http-api-stripped-lambda-permission-invoke-rejected|_http#18|Timeout|with urllib.request.urlopen(req, timeout=20) as resp:
Issue-Key: apigatewayv2.reset-authorizers-cache-server-side|_invoke#46|Timeout|with urllib.request.urlopen(req, timeout=timeout) as resp:
Issue-Key: apigatewayv2.route-throttling-returns-429-while-twin-route-serves|_http#17|Timeout|with urllib.request.urlopen(req, timeout=timeout_s) as resp:
Issue-Commit: 49d4e81f9c14b9ea7dd829b0faa6f7c6d506b5c4

# apigatewayv2: 7 net-new root causes where the execute-api data plane reaches no verdict — the WebSocket upgrade answers 404/hangs, and stage-deployment, CORS preflight, custom-domain, Lambda-permission, authorizer-cache and route-throttling invokes never settle

> _Note: this issue was identified against the code base at commit `49d4e81f9c14b9ea7dd829b0faa6f7c6d506b5c4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InvokeApi

## Description
Seven independent root causes, each one a group of behavioral checks that never reach an assertion because the emulator's execute-api data plane never answers (404 on the WebSocket handshake, or no response at all until the 60 s harness cap). None of these checks is marked ALREADY FILED or OWNED in this replay. Each cause must be closed separately.

Cause 1 of 7: a deployed WEBSOCKET stage never completes the RFC 6455 upgrade.
AWS: wss://{api-id}.execute-api.{region}.amazonaws.com/{stage} answers an RFC 6455 upgrade with 101 Switching Protocols (Sec-WebSocket-Accept derived from the client key); after that $connect/$disconnect and route-selection dispatch, @connections pushes, text/binary frame policy (binary frames refused with CLOSE 1003) and the 32 KiB client-message ceiling (CLOSE 1009) are all observable. Smithy: the control plane models ProtocolType=WEBSOCKET and RouteSelectionExpression; the upgrade itself is recorded behavior only.
Emulator: the handshake request answers 'HTTP/1.1 404 Not Found' (ConnectionError: websocket handshake failed) or the connection never settles and the check is killed at the 60 s cap, so no frame-level behavior is reachable. Open issues 20261104T160000Z-apigatewayv2-websocket-execute-api-upgrade-remains-unservable.md and 20261025T023548Z-apigatewayv2-websocket-stage-never-completes-the-rfc-6455.md describe the same front-door gap for other checks but do not carry these replay keys.
Why: a client cannot open a WebSocket at all — every duplex behavior (connect gating, action routing, broadcast, close codes) is unusable and untestable.
Fix: the execute-api front door's WebSocket upgrade seam plus the apigatewayv2 WEBSOCKET data-plane handler (handshake, connection registry, route dispatch).
Traces: apigatewayv2.client-initiated-close-invokes-disconnect-route, apigatewayv2.websocket-inbound-binary-frame-rejected, apigatewayv2.websocket-inbound-message-32kb-boundary (all three: 404 on handshake); apigatewayv2.websocket-broadcast-to-multiple-connections, apigatewayv2.websocket-connect-request-authorizer-token-gate, apigatewayv2.websocket-route-selection-by-action (hang to the cap).

Cause 2 of 7: stage/deployment semantics on the HTTP data plane never settle.
AWS: a non-autoDeploy stage pinned to a DeploymentId keeps serving that deployment's snapshot after the API is edited; a stage whose integration target comes from a stage variable is refused when the resolved Lambda has no invoke permission; and after DeleteStage the stage's invoke URL stops serving (404 {"message":"Not Found"}). Each is a response AWS returns promptly.
Emulator: each check hangs with no response until the 60 s cap — the stage's invoke URL never answers, so the snapshot, permission-refusal and post-delete legs reach no verdict.
Why: callers cannot rely on deployment pinning or on DeleteStage actually taking an endpoint out of service; a hung endpoint also stalls client timeouts instead of returning a status.
Fix: the HTTP data-plane stage resolution/serving path (deployment snapshot selection, stage-variable target resolution, stage deletion invalidating the route table) — it must always produce a response.
Traces: apigatewayv2.deep-apigatewayv2-3-stagesem-manual-stage-serves-deployment-snapshot, apigatewayv2.deep-apigatewayv2-3-stagesem-stage-variable-target-without-invoke-permission-rejected, apigatewayv2.delete-stage-stops-serving-requests.

Cause 3 of 7: the HTTP API CORS data plane never answers.
AWS: with a CorsConfiguration on the API (Smithy Api.CorsConfiguration, "A CORS configuration. Supported only for HTTP APIs"), the gateway answers an OPTIONS preflight itself with the configured Access-Control-Allow-Origin/-Methods/-Headers (refusing an origin outside AllowOrigins by omitting the header), overrides any CORS headers the backend sets on simple requests, and stops doing so once the configuration is deleted.
Emulator: both CORS checks hang to the 60 s cap — neither the preflight nor the simple request is answered.
Why: browser clients get no preflight answer at all (the request times out rather than being allowed or denied), so no browser-facing API can be exercised.
Fix: the HTTP data-plane CORS stage (preflight short-circuit for OPTIONS, and response-header override on the normal path).
Traces: apigatewayv2.http-api-cors-configuration-overrides-backend-cors-headers, apigatewayv2.http-api-cors-preflight-origin-allowlist-and-delete.

Cause 4 of 7: invoking an API through a custom domain name + API mapping never answers.
AWS: once a DomainName exists and an ApiMapping binds {api, stage, mappingKey} to it, a request to the domain (optionally under the mapping key) is routed to that stage and served exactly like the execute-api endpoint.
Emulator: the end-to-end custom-domain invoke hangs to the 60 s cap. The custom-domain CONTROL plane is covered by 20261026T023803Z-apigatewayv2-custom-domain-and-api-mapping-control-plane.md and 20261027T053233Z-apigatewayv2-9-net-new-root-causes-api-mapping.md; this check is not owned by either and fails on the data-plane invoke.
Why: every client addressing the API by its custom domain hangs instead of being served or refused.
Fix: the execute-api front door's host routing for custom domain names plus the API-mapping lookup in the HTTP data plane.
Traces: apigatewayv2.e2e-custom-domain-mapping-invoke.

Cause 5 of 7: a CDK HttpApi whose AWS::Lambda::Permission is stripped reaches no verdict.
AWS: removing the gateway's lambda:InvokeFunction permission makes the AWS_PROXY route fail closed — the gateway answers 500 {"message":"Internal Server Error"} for that route (and serves again once the permission is restored); the answer is prompt either way.
Emulator: the check hangs to the 60 s cap, so neither the refusal nor the restore is observable. Open issue 20261105T154551Z-apigatewayv2-a-cdk-httpapi-whose-aws-lambda-permission.md names the same scenario but does not carry this replay key.
Why: the resource-policy gate in front of Lambda integrations is unverifiable, and clients hang rather than receiving the documented 500.
Fix: the AWS_PROXY invoke path's permission check in the HTTP data plane (fail closed with a response rather than stalling).
Traces: apigatewayv2.deploy-apigatewayv2-3-cdk-http-api-stripped-lambda-permission-invoke-rejected.

Cause 6 of 7: the server-side effect of ResetAuthorizersCache is unobservable because the gated route never answers.
AWS: ResetAuthorizersCache flushes the stage's cached authorizer results, so the next request re-invokes the authorizer (a previously cached allow/deny no longer applies) — observable on the data plane within the stage's AuthorizerResultTtlInSeconds window.
Emulator: the check hangs to the 60 s cap — the authorizer-gated route never answers, so the cache cannot be observed before or after the reset. The control-plane response shape is covered by 20261106T165956Z-apigatewayv2-resetauthorizerscache-answers-204-and-createapimapping-mints-a.md; this is the data-plane side and is not owned by it.
Why: authorizer caching and its invalidation — a correctness- and cost-sensitive behavior — cannot be relied on at all.
Fix: the HTTP data-plane authorizer stage (result cache keyed per stage/identity source, invalidated by ResetAuthorizersCache) and whatever stalls the gated route.
Traces: apigatewayv2.reset-authorizers-cache-server-side.

Cause 7 of 7: per-route throttling never produces a 429.
AWS: Smithy RouteSettings.ThrottlingRateLimit/ThrottlingBurstLimit on a stage's RouteSettings cap that route; requests over the limit are refused with HTTP 429 and the gateway body {"message":"Too Many Requests"}, while a twin route without the setting keeps serving 200.
Emulator: the check hangs to the 60 s cap — neither the throttled route nor its twin answers.
Why: clients that implement retry/backoff on 429 can never be exercised, and a configured throttle silently protects nothing.
Fix: the HTTP data-plane throttling stage, applying the resolved RouteSettings token bucket per route and emitting 429 {"message":"Too Many Requests"}.
Traces: apigatewayv2.route-throttling-returns-429-while-twin-route-serves.

## Evidence
Replay verdicts and details, grouped by cause:

Cause 1 (WebSocket upgrade):
  [error] apigatewayv2.client-initiated-close-invokes-disconnect-route — ConnectionError: websocket handshake failed: b'HTTP/1.1 404 Not Found'
  [error] apigatewayv2.websocket-inbound-binary-frame-rejected — ConnectionError: websocket handshake failed: b'HTTP/1.1 404 Not Found'
  [error] apigatewayv2.websocket-inbound-message-32kb-boundary — ConnectionError: websocket handshake failed: b'HTTP/1.1 404 Not Found'
  [mismatch] apigatewayv2.websocket-broadcast-to-multiple-connections — Check timed out after 60.0s
  [mismatch] apigatewayv2.websocket-connect-request-authorizer-token-gate — Check timed out after 60.0s
  [mismatch] apigatewayv2.websocket-route-selection-by-action — Check timed out after 60.0s
Expected: 101 Switching Protocols on the upgrade, then frame-level dispatch. Actual: 404 Not Found / no response.

Cause 2 (stage semantics): all three timed out after 60.0s —
  apigatewayv2.deep-apigatewayv2-3-stagesem-manual-stage-serves-deployment-snapshot
  apigatewayv2.deep-apigatewayv2-3-stagesem-stage-variable-target-without-invoke-permission-rejected
  apigatewayv2.delete-stage-stops-serving-requests
Expected: the pinned stage serves the old snapshot (200), the unpermitted stage-variable target is refused, and the deleted stage answers 404 {"message":"Not Found"}. Actual: no response at all.

Cause 3 (CORS): both timed out after 60.0s —
  apigatewayv2.http-api-cors-configuration-overrides-backend-cors-headers
  apigatewayv2.http-api-cors-preflight-origin-allowlist-and-delete
Expected: a gateway-generated preflight response carrying the configured Access-Control-Allow-* headers (and no Allow-Origin for a disallowed origin). Actual: no response.

Cause 4: apigatewayv2.e2e-custom-domain-mapping-invoke — Check timed out after 60.0s. Expected the mapped stage's response through the custom domain host; actual no response.

Cause 5: apigatewayv2.deploy-apigatewayv2-3-cdk-http-api-stripped-lambda-permission-invoke-rejected — Check timed out after 60.0s. Expected 500 {"message":"Internal Server Error"} for the route whose invoke permission was stripped; actual no response.

Cause 6: apigatewayv2.reset-authorizers-cache-server-side — Check timed out after 60.0s. Expected the authorizer-gated route to answer (cached result before the reset, re-invocation after); actual no response.

Cause 7: apigatewayv2.route-throttling-returns-429-while-twin-route-serves — Check timed out after 60.0s. Expected 429 {"message":"Too Many Requests"} on the throttled route and 200 on the twin; actual no response.

## Steps to Reproduce
Replay the behavioral suite and re-run the checks listed in trace_ids.

Cause 1: CreateApi(ProtocolType=WEBSOCKET, RouteSelectionExpression='$request.body.action') with $connect/$disconnect/custom routes, deploy a stage, then perform an RFC 6455 upgrade against wss://{api-id}.execute-api.{region}.amazonaws.com/{stage}: expect 101 Switching Protocols, then exercise route dispatch, a client-initiated CLOSE invoking $disconnect, a binary frame (CLOSE 1003) and a >32 KiB message (CLOSE 1009).
Cause 2: (a) create a stage with AutoDeploy=false pinned to a DeploymentId, change the API, and confirm the stage still serves the pinned snapshot; (b) point an integration URI at a Lambda via a stage variable with no invoke permission and confirm a refusal; (c) DeleteStage and confirm its invoke URL now answers 404 {"message":"Not Found"}.
Cause 3: CreateApi with CorsConfiguration (AllowOrigins allowlist), send an OPTIONS preflight from an allowed and a disallowed Origin, send a simple request whose backend also sets CORS headers (gateway values must win), then delete the configuration and re-check.
Cause 4: CreateDomainName + CreateApiMapping binding a stage to the domain (with and without a mapping key), then invoke the API through the domain host.
Cause 5: deploy the CDK HttpApi stack with its AWS::Lambda::Permission removed and invoke the AWS_PROXY route; expect 500 {"message":"Internal Server Error"}.
Cause 6: attach a REQUEST authorizer with AuthorizerResultTtlInSeconds > 0, invoke the route twice (second served from cache), call ResetAuthorizersCache, and invoke again — the authorizer must be re-invoked.
Cause 7: set stage RouteSettings ThrottlingRateLimit/ThrottlingBurstLimit on one route, drive it above the limit, and expect 429 {"message":"Too Many Requests"} while an unthrottled twin route still answers 200.
