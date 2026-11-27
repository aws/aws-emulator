Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2.http-stage-access-log-format-over-3000-chars-rejected apigatewayv2.websocket-stage-access-log-format-over-3000-chars-rejected apigatewayv2.http-stage-access-log-format-without-request-id-rejected apigatewayv2.http-api-preflight-answered-from-cors-configuration-without-options-route apigatewayv2.http-proxy-integration-uri-path-variable-takes-the-route-value apigatewayv2.lambda-authorizer-payload-1-0-event-joins-identity-sources-into-a-token apigatewayv2.payload-1-0-repeated-header-is-single-valued-in-headers apigatewayv2.websocket-iam-authorization-outside-connect-route-rejected apigatewayv2.websocket-connect-api-key-required-handshake-without-key-rejected apigatewayv2.websocket-connect-disabled-api-key-rejected-until-reenabled
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigatewayv2.http-stage-access-log-format-over-3000-chars-rejected|_refusal#12|AssertionError|raise AssertionError(f"the call was accepted instead of refused: {answer!r}")
Issue-Key: apigatewayv2.websocket-stage-access-log-format-over-3000-chars-rejected|_error_of#11|AssertionError|raise AssertionError(f"expected a refusal, the call returned {result!r}")
Issue-Key: apigatewayv2.http-stage-access-log-format-without-request-id-rejected|run#11|AssertionError|assert denied == ("BadRequestException", MISSING_ID), (
Issue-Key: apigatewayv2.http-api-preflight-answered-from-cors-configuration-without-options-route|run#15|AssertionError|assert (status, body, _cors(hdrs)) == (404, _NOT_FOUND, {}), (
Issue-Key: apigatewayv2.http-proxy-integration-uri-path-variable-takes-the-route-value|run#7|AssertionError|assert event.get("rawPath") == "/pets/77", (
Issue-Key: apigatewayv2.lambda-authorizer-payload-1-0-event-joins-identity-sources-into-a-token|run#13|AssertionError|assert seen.get("identitySource") == "user1", (
Issue-Key: apigatewayv2.payload-1-0-repeated-header-is-single-valued-in-headers|run#21|AssertionError|assert single.get("x-dup") == "two", (
Issue-Key: apigatewayv2.websocket-iam-authorization-outside-connect-route-rejected|_refusal#7|AssertionError|raise AssertionError(f"the call was accepted where a refusal was expected: {out!r}")
Issue-Key: api-gateway|CreateUsagePlan|NotFoundException
Issue-Commit: af007de286377294faf8721f963c7a04ad7dcdf8

# apigatewayv2: 8 net-new causes — access-log format limit and request-id refusals, CORS headers on a route-miss 404, HTTP_PROXY path variables, payload-1.0 header/identity-source shapes, AWS_IAM on non-$connect routes, and usage plans over a v2 API

> _Note: this issue was identified against the code base at commit `af007de286377294faf8721f963c7a04ad7dcdf8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStage

## Description
Eight net-new root causes in the apigatewayv2 control and data planes. Each is numbered with its own failing check ids; all eight must be closed before this issue is submitted.

Cause 1 of 8: the 3000-character ceiling on AccessLogSettings.Format is not enforced.
AWS: recorded behavior — a format of exactly 3000 characters is stored and read back unchanged, and one character more is refused with HTTP 400 `BadRequestException` whose message is the fixed text "The following errors are found in Access Log Format: Access log format must be less than or equal to 3000 characters" (no variable part), on both CreateStage and UpdateStage and for both HTTP and WEBSOCKET stages; the refused call leaves the stage with no AccessLogSettings. (Smithy types AccessLogSettings$Format as StringWithLengthBetween1And1024 and documents only "must include at least $context.requestId", so the ceiling is the recorded one, not the model's.)
Emulator: the 3001-character format is ACCEPTED — UpdateStage answers 200 and GetStage reports the oversize format (HTTP stage), and the WEBSOCKET stage create answers 201 with it stored.
Why: a client that relies on AWS refusing an oversize format silently gets a stage whose log format AWS would never have accepted, so the configuration cannot be ported back to AWS and the refusal path is untestable.
Fix: the AccessLogSettings validation shared by the CreateStage and UpdateStage handlers in the apigatewayv2 service crate.
Traces: apigatewayv2.http-stage-access-log-format-over-3000-chars-rejected, apigatewayv2.websocket-stage-access-log-format-over-3000-chars-rejected.

Cause 2 of 8: the refusal for an access-log format naming no request id is the emulator's own sentence, and $context.extendedRequestId is not accepted as the id.
AWS: recorded behavior — CreateStage/UpdateStage with a Format that names neither $context.requestId nor $context.extendedRequestId is refused HTTP 400 `BadRequestException` with the fixed text "The following errors are found in Access Log Format: Access Log format must include either $context.requestId or $context.extendedRequestId : " followed by the refused format value quoted from the request; nothing is stored. A format carrying $context.extendedRequestId instead is accepted.
Emulator: `('BadRequestException', "Invalid value for AccessLogSettings: the Format must include at least $context.requestId, got '$context.httpMethod $context.routeKey $context.status'.")` — right code, wrong fixed text, and it names only $context.requestId. Hypothesis: the same predicate also refuses an extendedRequestId-only format, which the check could not reach past the first assertion.
Why: a client matching AWS's message (or relying on extendedRequestId being a valid id) sees an unknown sentence and, in the extendedRequestId case, a refusal where AWS accepts.
Fix: the AccessLogSettings format predicate and its message in the CreateStage/UpdateStage validation of the apigatewayv2 crate. The same defect appears under the apigatewayv2__logs heading (filed separately for that service token) via update-stage-access-log-format-without-a-request-id-is-rejected.
Traces: apigatewayv2.http-stage-access-log-format-without-request-id-rejected.

Cause 3 of 8: the gateway's own 404 route-miss response carries CORS headers.
AWS: for an HTTP API with a CorsConfiguration, a request that matches NO route is answered 404 with body {"message":"Not Found"} and NO Access-Control-* headers — CORS headers are added only to responses of a matched route (and to the preflight the gateway answers for OPTIONS).
Emulator: the 404 body is right but the response carries `access-control-allow-origin: https://shop.example.com` and `access-control-expose-headers: x-order-id`.
Why: a browser client is told a cross-origin read of a nonexistent route is permitted, so a route typo surfaces as a readable 404 instead of a CORS failure; the two environments disagree on what the browser sees.
Fix: the execute-api front door's CORS response decoration — apply it after route selection succeeds, not to gateway-generated route-miss responses.
Traces: apigatewayv2.http-api-preflight-answered-from-cors-configuration-without-options-route.

Cause 4 of 8: HTTP_PROXY integration URI path variables are not substituted.
AWS: an HTTP_PROXY integration whose IntegrationUri contains a route path variable (e.g. .../pets/{id}) is resolved per request by substituting the matched route's path parameter, so the backend receives /pets/77 for GET /pets/{id} with id=77.
Emulator: the backend receives the literal '/pets/{id}'.
Why: every proxied request reaches the wrong backend path; backends 404 or mis-handle a literal brace path.
Fix: the HTTP_PROXY request builder in the execute-api data plane — IntegrationUri templating against the selected route's path parameters.
Traces: apigatewayv2.http-proxy-integration-uri-path-variable-takes-the-route-value.

Cause 5 of 8: a payload-format-1.0 Lambda authorizer event carries identitySource as a list instead of the joined string.
AWS: recorded behavior — for a REQUEST authorizer with payload format 1.0 the event's `identitySource` is a plain string: the configured identity sources' values joined, e.g. 'user1' for one source.
Emulator: `['user1']` — a JSON array.
Why: an authorizer function that treats identitySource as a token string (splitting or comparing it) crashes or denies on the emulator while succeeding on AWS.
Fix: the Lambda-authorizer payload-1.0 event builder in the apigatewayv2 data plane.
Traces: apigatewayv2.lambda-authorizer-payload-1-0-event-joins-identity-sources-into-a-token.

Cause 6 of 8: a repeated request header is comma-joined in the payload-1.0 `headers` map instead of carrying the last value.
AWS: under payload format 1.0 the single-valued `headers` map holds the LAST value sent for a repeated header (every value is listed under `multiValueHeaders`).
Emulator: headers['x-dup'] is 'one,two' where AWS gives 'two'.
Why: a 1.0 function reading event.headers sees a synthetic comma-joined value that is not any value the client sent, so header-driven dispatch and signature/ETag comparisons differ from AWS.
Fix: the payload-1.0 event builder's headers/multiValueHeaders construction.
Traces: apigatewayv2.payload-1-0-repeated-header-is-single-valued-in-headers.

Cause 7 of 8: AWS_IAM authorization is accepted on a WebSocket route other than $connect.
AWS: for a WEBSOCKET API, authorization is settable only on $connect; CreateRoute or UpdateRoute setting AuthorizationType=AWS_IAM on a message route is refused HTTP 400 `BadRequestException` with the fixed text "Currently, authorization is restricted to the $connect route only", and the route keeps the authorization it had. (Smithy: AuthorizationType enum NONE|AWS_IAM|CUSTOM|JWT; CreateRoute/UpdateRoute error BadRequestException 400.)
Emulator: CreateRoute(RouteKey='orders', AuthorizationType='AWS_IAM') answers 201 with AuthorizationType 'AWS_IAM'.
Why: a template or SDK caller gets an API whose message routes claim IAM authorization that AWS would have refused outright — the configuration is unportable and the apparent protection is not AWS's model.
Fix: CreateRoute/UpdateRoute validation in the apigatewayv2 crate — the WEBSOCKET $connect-only authorization rule.
Traces: apigatewayv2.websocket-iam-authorization-outside-connect-route-rejected.

Cause 8 of 8: API Gateway CreateUsagePlan refuses an apiStages entry naming an apigatewayv2 (WebSocket) API id.
AWS: a WebSocket API's ApiKeyRequired routes are metered through an API Gateway usage plan, so CreateUsagePlan accepts an apiStages entry whose apiId is the v2 API id and whose stage is that API's stage, and returns the plan.
Emulator: `NotFoundException: An error occurred (NotFoundException) when calling the CreateUsagePlan operation: Invalid API identifier specified <account>:<api-id>` — the v2 API id is unknown to the usage-plan handler, which blocks both API-key handshake checks before any assertion.
Why: a client cannot build the API-key path for a WebSocket API at all; the whole api-key-required / key-disabled handshake contract is unreachable.
Fix: the CreateUsagePlan handler's apiStages resolution — resolve the apiId against apigatewayv2 APIs as well as REST APIs.
Traces: apigatewayv2.websocket-connect-api-key-required-handshake-without-key-rejected, apigatewayv2.websocket-connect-disabled-api-key-rejected-until-reenabled.

## Evidence
Cause 1: expected ('BadRequestException', 'The following errors are found in Access Log Format: Access log format must be less than or equal to 3000 characters'); got HTTP 200/201 with the 3001-character Format stored (AssertionError: "the call was accepted instead of refused: {... 'AccessLogSettings': {... 'Format': '{\"rid\":\"$context.requestId\",\"pad\":\"xxxx...'}}"; and "expected a refusal, the call returned {...'Format': '$context.requestId\t$context.routeKey\twwww...'}").
Cause 2: expected ('BadRequestException', "The following errors are found in Access Log Format: Access Log format must include either $context.requestId or $context.extendedRequestId : '<format>'"); got ('BadRequestException', "Invalid value for AccessLogSettings: the Format must include at least $context.requestId, got '<format>'.").
Cause 3: expected (404, '{"message":"Not Found"}', {}); got 404 with {'access-control-allow-origin': 'https://shop.example.com', 'access-control-expose-headers': 'x-order-id'}.
Cause 4: expected backend path '/pets/77'; got '/pets/{id}'.
Cause 5: expected identitySource 'user1' (str); got ['user1'].
Cause 6: expected headers['x-dup'] == 'two'; got 'one,two'.
Cause 7: expected ('BadRequestException', 'Currently, authorization is restricted to the $connect route only'); got HTTP 201 {'AuthorizationType': 'AWS_IAM', 'RouteKey': 'orders', ...}.
Cause 8: expected CreateUsagePlan to return a plan for apiStages=[{apiId: <v2 api id>, stage: <stage>}]; got botocore.errorfactory.NotFoundException 'Invalid API identifier specified 000000000000:<api-id>'.

## Steps to Reproduce
Cause 1: CreateStage, then UpdateStage with AccessLogSettings.Format of 3001 characters (HTTP) and CreateStage with one of 3001 characters (WEBSOCKET) — both are accepted instead of 400.
Cause 2: CreateStage with AccessLogSettings.Format='$context.httpMethod $context.routeKey $context.status' — compare code and message with AWS's; then retry with '$context.extendedRequestId' appended and check it is accepted.
Cause 3: CreateApi(ProtocolType='HTTP', CorsConfiguration={AllowOrigins:['https://shop.example.com'], ExposeHeaders:['x-order-id'], ...}), auto-deploy $default, GET a path matching no route with an Origin header — the 404 carries Access-Control-* headers.
Cause 4: CreateIntegration(IntegrationType='HTTP_PROXY', IntegrationUri='http://backend/pets/{id}'), CreateRoute('GET /pets/{id}'), GET /pets/77 — backend sees '/pets/{id}'.
Cause 5: CreateAuthorizer(AuthorizerType='REQUEST', AuthorizerPayloadFormatVersion='1.0', IdentitySource=['$request.header.x-user']), invoke the route — event.identitySource is a list.
Cause 6: send a request with the same header twice under payload 1.0 — event.headers holds 'one,two'.
Cause 7: CreateApi(ProtocolType='WEBSOCKET'), CreateRoute(RouteKey='$connect', AuthorizationType='AWS_IAM'), then CreateRoute(RouteKey='orders', AuthorizationType='AWS_IAM') and UpdateRoute of an existing NONE route to AWS_IAM — both are accepted.
Cause 8: CreateApi(ProtocolType='WEBSOCKET'), CreateStage/CreateDeployment, then apigateway CreateUsagePlan(apiStages=[{'apiId': <v2 api id>, 'stage': <stage>}]) — NotFoundException.
