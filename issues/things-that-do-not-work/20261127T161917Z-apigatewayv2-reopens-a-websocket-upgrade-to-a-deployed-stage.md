Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2.websocket-128kb-message-in-32kb-frames-reaches-integration-whole apigatewayv2.websocket-connect-integration-any-2xx-status-upgrades apigatewayv2.websocket-connect-returned-subprotocol-names-the-upgrade apigatewayv2.websocket-iam-connect-signed-upgrade-is-served-as-the-role apigatewayv2.websocket-iam-connect-unsigned-upgrade-rejected apigatewayv2.websocket-nested-route-selection-expression-dispatches-each-frame apigatewayv2.websocket-post-to-client-closed-connection-rejected-gone apigatewayv2.websocket-post-to-connection-delivers-exact-bytes-to-that-socket-only apigatewayv2.websocket-request-template-past-300kb-still-evaluated
Issue-Key: apigatewayv2.websocket-128kb-message-in-32kb-frames-reaches-integration-whole|_open#12|AssertionError|raise AssertionError(f"the upgrade was never served: {exc!r}") from exc
Issue-Key: apigatewayv2.websocket-connect-integration-any-2xx-status-upgrades|_open#8|AssertionError|raise AssertionError(f"the upgrade was never served: {exc!r}") from exc
Issue-Key: apigatewayv2.websocket-connect-returned-subprotocol-names-the-upgrade|_open#10|AssertionError|raise AssertionError(f"the upgrade was never served: {exc!r}") from exc
Issue-Key: apigatewayv2.websocket-iam-connect-signed-upgrade-is-served-as-the-role|_signed_connect#19|AssertionError|raise AssertionError(f"the signed upgrade was never accepted: {exc!r}") from exc
Issue-Key: apigatewayv2.websocket-iam-connect-unsigned-upgrade-rejected|_signed_connect#18|AssertionError|raise AssertionError(f"the signed upgrade was never accepted: {exc!r}") from exc
Issue-Key: apigatewayv2.websocket-nested-route-selection-expression-dispatches-each-frame|_connect#10|TimeoutError|raise TimeoutError(
Issue-Key: apigatewayv2.websocket-post-to-client-closed-connection-rejected-gone|_connect#11|TimeoutError|raise TimeoutError(
Issue-Key: apigatewayv2.websocket-post-to-connection-delivers-exact-bytes-to-that-socket-only|_connect#12|TimeoutError|raise TimeoutError(
Issue-Key: apigatewayv2.websocket-request-template-past-300kb-still-evaluated|_connect#23|Timeout|time.sleep(min(2 ** attempt, 20))
Issue-Commit: af007de286377294faf8721f963c7a04ad7dcdf8

# REOPENS: a WebSocket upgrade to a deployed stage at the announced ApiEndpoint is refused 403 Forbidden instead of 101 Switching Protocols (1 cause, 9 checks)

> _Note: this issue was identified against the code base at commit `af007de286377294faf8721f963c7a04ad7dcdf8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
WebSocket upgrade (execute-api data plane)

## Description
REOPENS 20261107T120000Z-apigatewayv2-the-front-door-cannot-switch-a-websocket-handshake-to-the-apis-own-socket.md: its fix left the call refused.

Cause 1 of 1: an RFC 6455 opening handshake sent to a deployed WEBSOCKET stage, at the address the control plane publishes for that API, is answered 403 Forbidden with a JSON content-type instead of being switched to 101.

AWS: a WEBSOCKET API's stage, reached at the `ApiEndpoint` the control plane reports with the stage name appended (Smithy: Api$ApiEndpoint is "The URI of the API, of the form {api-id}.execute-api.{region}.amazonaws.com"; ProtocolType WEBSOCKET, RouteSelectionExpression required at CreateApi), answers a correct upgrade with `101 Switching Protocols`, the `Sec-WebSocket-Accept` of the client's key, and the negotiated subprotocol when one was offered; it then dispatches each frame through the route-selection expression and registers the connection so @connections PostToConnection reaches it. A $connect integration answering any 2xx lets the upgrade through; a 403 is reserved for a $connect that is refused (authorizer deny / unauthorized SigV4), and the AWS_IAM $connect route accepts a correctly SigV4-signed handshake.

Emulator: every handshake in these nine checks is answered `403 Forbidden` with headers `{'content-type': 'application/json'}` and no body (`WebSocketBadStatusException("Handshake status 403 Forbidden ...")`), including the healthy control legs the checks use to establish the socket before asserting anything, and including the SIGNED AWS_IAM $connect handshake (`websocket-iam-connect-signed-upgrade-is-served-as-the-role`, `...-unsigned-upgrade-rejected`, whose control leg reports "the signed upgrade was never accepted"). The checks dial exactly the value the control plane reported for the API (via the harness's stage_url of `ApiEndpoint`), which the emulator currently publishes as a path-style loopback URL `https://<host>:<port>/_aws/execute-api/<api-id>` (see open issue 20261127T014922Z-apigatewayv2-cloudcontrol-cloudformation-apigatewayv2-getapi-publishes-apiendpoint-as-a-path-style.md) — i.e. the address the fixed issue above calls the emulator-internal one, where it documents 403 as "a stage the endpoint does not serve". Whichever address is intended to be the published one, the result on the wire is a refusal where AWS switches protocols. `websocket-request-template-past-300kb-still-evaluated` reaches no verdict at all (timed out at the 60 s cap) and is grouped here because it, too, cannot get past the stage handshake.

Why: a client that follows the published `ApiEndpoint` can never open a WebSocket connection, so NOTHING of the WebSocket data plane is observable: frame dispatch by route-selection expression (including nested expressions), message reassembly of a 128 KiB message sent in 32 KiB frames, a $connect integration's 2xx admitting the upgrade, subprotocol negotiation, AWS_IAM $connect authorization (signed accepted / unsigned refused), @connections delivery to exactly one socket, and the GoneException after the client closes. Code 403 also mis-signals: a client treats it as an authorization failure and will not retry or re-sign correctly.

Fix: the WEBSOCKET execute-api data plane's upgrade path in the apigatewayv2 service crate — the handshake handler that decides between switching the connection and refusing it, and its agreement with the address CreateApi/GetApi publish as `ApiEndpoint` (service/websocket transport plus the api-endpoint reporter), so the published address is the one that answers 101.

Traces: the nine checks in trace_ids; the related open issues 20261104T160000Z-apigatewayv2-websocket-execute-api-upgrade-remains-unservable.md and 20261111T090000Z-apigatewayv2-iam-lambda-logs-the-front-door-cannot-upgrade-the-connect-handshake.md describe earlier 404/500/501 shapes of this path and do not carry these replay keys.

## Evidence
Expected: HTTP 101 Switching Protocols on the opening handshake to <ApiEndpoint>/<stage>, then frames dispatched/registered.
Actual (all nine): WebSocketBadStatusException("Handshake status 403 Forbidden -+-+- {'content-type': 'application/json'} -+-+- None"); the harness reports e.g. "the upgrade was never served", "the signed upgrade was never accepted", "TimeoutError: the WebSocket handshake to the stage never succeeded ... the run budget ended first; not graded".
One check (websocket-request-template-past-300kb-still-evaluated) ends only as "Check timed out after 60.0s".

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. CreateApi(ProtocolType='WEBSOCKET', RouteSelectionExpression='$request.body.action'); CreateIntegration; CreateRoute(RouteKey='$connect'|'$default', Target='integrations/<id>'); CreateStage(StageName='live'); CreateDeployment(StageName='live').
3. Read ApiEndpoint back from GetApi and append the stage name, exactly as Api$ApiEndpoint documents.
4. Send an RFC 6455 opening handshake to that URL: observe 403 Forbidden with content-type application/json instead of 101 Switching Protocols. Repeat with a correctly SigV4-signed handshake against an AWS_IAM $connect route: still 403.
