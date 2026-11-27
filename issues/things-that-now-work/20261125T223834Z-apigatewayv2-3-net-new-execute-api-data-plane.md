Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2.deep-apigatewayv2-2-enforce-disable-execute-api-endpoint-404-and-restore apigatewayv2.route-throttling-returns-429-while-twin-route-serves apigatewayv2.deploy-apigatewayv2-1-http-api-cors-removed-in-place-then-api-replaced
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigatewayv2.deep-apigatewayv2-2-enforce-disable-execute-api-endpoint-404-and-restore|_wait_status#24|Timeout|time.sleep(interval_s)
Issue-Key: apigatewayv2.route-throttling-returns-429-while-twin-route-serves|_poll_until#17|Timeout|time.sleep(interval_s)
Issue-Key: apigatewayv2.deploy-apigatewayv2-1-http-api-cors-removed-in-place-then-api-replaced|_poll_http#65|Timeout|time.sleep(interval_s)
Issue-Commit: 5de8975540ca22ec41f789488a788a9a12b974db

# apigatewayv2: 3 net-new execute-api data-plane causes reach no verdict — the disabled default endpoint never answers 404 Not Found, a per-route throttle never sheds a burst with 429, and an Origin-bearing GET on a CFN-deployed CORS API never serves

> _Note: this issue was identified against the code base at commit `5de8975540ca22ec41f789488a788a9a12b974db`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateApi

## Description
Three net-new root causes in the apigatewayv2 execute-api data plane, all grounded in failing behavioral checks that are NOT marked ALREADY FILED and NOT marked OWNED by any open issue in this replay. A fixer must close ALL THREE before submitting.

=== Cause 1 of 3: an HTTP API whose default execute-api endpoint has been disabled never answers the documented 404 {"message":"Not Found"} refusal, so neither the disabled state nor the restore back to serving can be observed. Failing check: apigatewayv2.deep-apigatewayv2-2-enforce-disable-execute-api-endpoint-404-and-restore.

AWS: Smithy (services/service-com-amazonaws-apigatewayv2-apigatewayv2/smithy-model/com-amazonaws-apigatewayv2-apigatewayv2.json, com.amazonaws.apigatewayv2#ApiGatewayV2): UpdateApi (PATCH /v2/apis/{ApiId}, 200) takes DisableExecuteApiEndpoint (__boolean, jsonName disableExecuteApiEndpoint) and echoes it in UpdateApiResponse; GetApi reports the same member, so the flag is per-API state readable right after the update. UpdateApi's modelled errors are BadRequestException 400, ConflictException 409, NotFoundException 404, TooManyRequestsException 429 — none applies, the control-plane call succeeds. The data-plane consequence is recorded behavior only (the execute-api data plane is not modelled in Smithy): once the flag is set, every request to that API's default execute-api URL is refused HTTP 404 with the gateway envelope body {"message":"Not Found"} — that body is entirely fixed text, it carries no value from the request or from state — and the integration is NOT invoked, so nothing of a refused request reaches the backend (no log line, no backend body). The refusal is scoped to the one API: a twin API over the same backend with the flag clear keeps answering 200 with the backend's body in the same instant. Clearing the member (DisableExecuteApiEndpoint=false) returns the same URL to 200 from the integration.

Emulator: the check reaches no verdict — replay detail 'Error: Check timed out after 60.0s'. After the successful UpdateApi the check polls the target URL for a settled run of 404s (and, after re-enabling, for a settled run of 200s); neither run materialises inside the harness budget, so the whole disable/restore transition is ungradeable. Hypothesis: the front door's disabled-endpoint gate answers a refusal class other than 404 {"message":"Not Found"} (an earlier filing records 403 {"message":"Forbidden"} from this gate), so the status the poll waits for never appears; the model and the recording state only that the documented refusal is 404 with that envelope.

Why: a client cannot tell that disabling an API's default endpoint took effect the way AWS defines it, nor that re-enabling restored it. Endpoint-lockdown and custom-domain-only cutovers key on 404/{"message":"Not Found"} to separate 'the default endpoint is off' from an authorization failure or a missing route; against the emulator that signal never arrives and the restore path is unobservable.

Fix: the execute-api front door's DisableExecuteApiEndpoint gate for HTTP APIs — the status and envelope it answers for a request to a disabled default endpoint, refusing BEFORE invoking the integration, its per-API scoping (an enabled twin must be unaffected), and the path back to serving when UpdateApi clears the member. Prior art, none of which the suite attributed this check to: open 20261122T132825Z-apigatewayv2-an-http-api-with-disableexecuteapiendpoint-true-never.md (same cause, re-filed here because the replay did not mark the check OWNED by it), open 20261112T190000Z-apigatewayv2-the-disableexecuteapiendpoint-check-still-hangs-on-the-front-door-upgrade.md, and the DECLINED cause of fixed 20261125T111135Z-apigatewayv2-3-net-new-causes-http-proxy-request.md, whose resolution argues the behaviour is already correct in-process; the replayed check still produces no verdict, so the behaviour at the published address is still not demonstrated.

Traces: apigatewayv2.deep-apigatewayv2-2-enforce-disable-execute-api-endpoint-404-and-restore.

=== Cause 2 of 3: a per-route throttle on a live stage never sheds any part of an over-limit burst with 429, and the routes under test never settle into a gradeable state, so the throttling leg reaches no verdict. Failing check: apigatewayv2.route-throttling-returns-429-while-twin-route-serves.

AWS: Smithy: CreateStage (POST /v2/apis/{ApiId}/stages, 201) takes RouteSettings (RouteSettingsMap, jsonName routeSettings) and DefaultRouteSettings (RouteSettings, jsonName defaultRouteSettings); shape RouteSettings carries ThrottlingRateLimit (__double, doc 'Specifies the throttling rate limit') and ThrottlingBurstLimit (__integer, doc 'Specifies the throttling burst limit'); DeleteRouteSettings (DELETE /v2/apis/{ApiId}/stages/{StageName}/routesettings/{RouteKey}, 204, errors NotFoundException 404 / TooManyRequestsException 429) removes one route's override. Enforcement is recorded behavior only (docs: https://docs.aws.amazon.com/apigateway/latest/developerguide/http-api-throttling.html): a route whose settings cap it far below the offered rate has part of an over-limit burst shed by the gateway with HTTP 429 and the gateway's 'Too Many Requests' envelope — that text is fixed, no value from the request appears in it — shed BEFORE the integration runs, so no backend body appears in a throttled response, while the remainder of the burst is served normally 200 from the integration. The limit is scoped to the one RouteKey: a twin route on the same stage and the same integration, under the stage's generous DefaultRouteSettings, sheds nothing in the same instant. After DeleteRouteSettings the same burst against that route sheds nothing.

Emulator: the check reaches no verdict — replay detail 'Error: Check timed out after 60.0s'. Its legs are, in order, a poll for each of 'GET /limited' and 'GET /twin' answering 200 with the backend marker, then a slow run of five requests to each route that must ALL answer 200 with that marker, then an interleaved 24-request burst expecting at least one 429 on the limited route and none on the twin. No settled state the check waits for is reached inside the harness budget. Hypothesis (the recording and the model state only the contract above): either the front door applies no per-route meter at all, so the state the burst polls toward never appears, or the meter sheds the slow pre-burst run as well (its earlier filing's replay frame was inside the run poll, _poll_until), which a 1 rps / burst-1 route must NOT do for requests spaced seconds apart.

Why: a client cannot verify that a per-route throttle protects a backend, and cannot distinguish 'shed by the gateway' (429 + Too Many Requests, integration not invoked) from a backend error. Load-shedding and quota-regression suites pass against the emulator while the same stage configuration sheds traffic on AWS — or, in the other direction, a correctly-paced caller is refused where AWS serves it.

Fix: the execute-api front door's rate-limiting stage — the per-route and per-stage token buckets derived from a stage's RouteSettings/DefaultRouteSettings (refill at ThrottlingRateLimit, depth ThrottlingBurstLimit), the 429 'Too Many Requests' refusal emitted before the integration is invoked, the per-RouteKey scoping against the stage default, and DeleteRouteSettings taking effect on a live AutoDeploy stage. Prior art, not carrying this check's key in this replay: the DECLINED Cause 2 of fixed 20261125T111135Z-apigatewayv2-3-net-new-causes-http-proxy-request.md and fixed 20261114T024946Z-apigatewayv2-3-net-new-causes-a-disabled-execute.md; the check still produces no verdict at the published address.

Traces: apigatewayv2.route-throttling-returns-429-while-twin-route-serves.

=== Cause 3 of 3: a CloudFormation-deployed HTTP API with a CorsConfiguration never serves an Origin-bearing GET on its $default AutoDeploy stage, so the in-place removal of CORS and the subsequent Api replacement are both ungradeable. Failing check: apigatewayv2.deploy-apigatewayv2-1-http-api-cors-removed-in-place-then-api-replaced.

AWS: Smithy: CreateApi/UpdateApi member CorsConfiguration : Cors — AllowOrigins (CorsOriginList, jsonName allowOrigins), AllowMethods (jsonName allowMethods), AllowHeaders, ExposeHeaders, AllowCredentials, MaxAge (@range -1..86400), every member documented 'Supported only for HTTP APIs', the shape doc linking https://docs.aws.amazon.com/apigateway/latest/developerguide/http-api-cors.html. The data plane is recorded behavior only, and CORS is a BROWSER mechanism: API Gateway never refuses a request because of its Origin. A GET on a route with AuthorizationType NONE is served 200 with the integration's body whether or not an Origin header is present; the only Origin-dependent difference is the RESPONSE decoration — access-control-allow-origin equal to the request's Origin when that origin is in AllowOrigins, and no access-control-* header at all when it is not. An OPTIONS preflight (Origin + Access-Control-Request-Method) that no route matches is answered by the gateway itself 204 with access-control-allow-origin / -allow-methods / -allow-headers / -max-age taken from the API's own CorsConfiguration; once CorsConfiguration is removed and no OPTIONS route exists, the same preflight falls through to route matching and is answered 404 with body {"message":"Not Found"} and NO access-control-* header, while the plain GET keeps answering 200 with the integration's body and no CORS headers. In all of this the statuses, the header names and the JSON error shape are the fixed part; the origin string, the method/header lists, the max-age number and the handler's marker are values taken from the request and from the API's own configuration.

Emulator: the check reaches no verdict — replay detail 'Error: Check timed out after 60.0s'. Its first data-plane leg polls the stack's $default stage URL for 200 on 'GET /a' WITH header Origin: <the stack's allow-listed origin> (_poll_http(url, accept=200, headers={'Origin': ...})); that 200 never arrives inside the poll, so every later leg — the preflight 204 with the configured MaxAge, the in-place stack update that removes CorsConfiguration, the preflight then falling through to 404 with no access-control-allow-origin, the GET losing its CORS headers, and the Route-repointing replacement that must yield a new ApiId with the old one NotFound — is never exercised. Hypothesis: the same front-door same-origin gate that open issue 20261124T103252Z-apigatewayv2-the-execute-api-front-door-still-refuses.md records, which answers any Origin-bearing execute-api request HTTP 403, content-type 'text/plain; charset=utf-8', body 'cross-site origin' before route matching, the authorizer, the integration or the API's CorsConfiguration is consulted. That open issue carries four other CORS checks' replay keys and does NOT carry this one, so this check is filed here rather than dropped; a fixer may close them together.

Why: the very requests CORS exists to permit are destroyed — a browser page or any client that forwards Origin cannot call an HTTP API whose allowlist explicitly names its origin — and server-side Origin enforcement is not a CORS mechanism, so an application written against the emulator believes AllowOrigins is an access control when on AWS the request is served regardless and only response headers differ. In addition the invented plain-text body breaks every client that parses API Gateway's {"message":...} error shape, and the whole CFN lifecycle this check grades (CORS removed in place keeping the ApiId, then the Api replaced) becomes unobservable.

Fix: the shared HTTP front-door/edge middleware that applies the same-origin gate ahead of the apigatewayv2 execute-api data plane — remove the server-side Origin check so the request proceeds to stage/route resolution, the route's authorizer and its integration, and let the API's CorsConfiguration shape only the RESPONSE (the cors decoration path in the apigatewayv2 invoke module) on the 200, the 204 preflight and the 404 {"message":"Not Found"} fall-through alike.

Traces: apigatewayv2.deploy-apigatewayv2-1-http-api-cors-removed-in-place-then-api-replaced.

## Evidence
Cause 1 — check apigatewayv2.deep-apigatewayv2-2-enforce-disable-execute-api-endpoint-404-and-restore, verdict mismatch, detail 'Error: Check timed out after 60.0s'.
Expected (AWS): after UpdateApi(ApiId=<target>, DisableExecuteApiEndpoint=True) succeeds and GetApi reports DisableExecuteApiEndpoint=true on the target and false on the twin, every GET of the target's execute-api URL answers HTTP 404 with body {"message":"Not Found"} (fixed text, no variable part) and never the backend's body; the twin API's URL answers 200 with the backend's body in the same instant; no refused request's nonce ever appears in the backend's log (the integration was not invoked); then after UpdateApi(DisableExecuteApiEndpoint=False), GetApi reports false and the same URL answers 200 with the backend's body again, with that request visible in the backend's log.
Actual (emulator): no verdict inside the harness cap — the settled run of 404s the check polls for on the disabled target URL is never answered, so the restore leg is never reached. The 404 status and the {"message":"Not Found"} envelope are the requirement; the API ids, the generated marker and the per-request nonces are per-run environment values.

Cause 2 — check apigatewayv2.route-throttling-returns-429-while-twin-route-serves, verdict mismatch, detail 'Error: Check timed out after 60.0s'.
Expected (AWS): on a $default AutoDeploy stage with DefaultRouteSettings {ThrottlingRateLimit 500.0, ThrottlingBurstLimit 200} and RouteSettings {'GET /limited': {ThrottlingRateLimit 1.0, ThrottlingBurstLimit 1}} (GetStage reports the override on 'GET /limited' only), requests spaced seconds apart are served 200 with the backend's body on BOTH routes; an interleaved ~24-request burst then sheds at least one request on GET /limited with HTTP 429 whose body carries 'Too Many Requests' and never the backend's body, serves the rest 200 from the backend, and sheds nothing on GET /twin in the same instant; after DeleteRouteSettings(RouteKey='GET /limited') the same burst sheds nothing.
Actual (emulator): no verdict inside the harness cap — neither the pre-burst run in which every request to both routes must answer 200 with the marker, nor any 429 on the limited route, is ever observed. The 429 status and the 'Too Many Requests' envelope are the requirement; the marker string, the API id and the exact shed counts are environment values (the check asserts only '>= 1 shed on the limited route, 0 on the twin').

Cause 3 — check apigatewayv2.deploy-apigatewayv2-1-http-api-cors-removed-in-place-then-api-replaced, verdict mismatch, detail 'Error: Check timed out after 60.0s'.
Expected (AWS): a CFN stack creating an HTTP API with CorsConfiguration {AllowOrigins:[<allowed origin>], AllowMethods:['GET'], MaxAge:300}, an AWS_PROXY Lambda integration, route 'GET /a' and a $default AutoDeploy stage serves GET /a with header Origin: <allowed origin> as 200, body carrying the Lambda's marker, routeKey 'GET /a' and version '2.0', plus access-control-allow-origin == <allowed origin>; an OPTIONS preflight is 204 with that origin echoed, 'GET' in access-control-allow-methods and access-control-max-age == the configured MaxAge; a stack update that only removes CorsConfiguration and changes Description is an in-place Modify (Replacement False) that keeps the ApiId, GetApi no longer returns CorsConfiguration, the same preflight now falls through to 404 with no access-control-allow-origin, and GET /a still answers 200 with the marker and no CORS headers.
Actual (emulator): no verdict inside the harness cap — the very first Origin-bearing GET never reaches 200, so nothing after it runs. Per the related open filing, the emulator answers such a request HTTP 403 with content-type 'text/plain; charset=utf-8' and body 'cross-site origin' — a status, body and content type AWS never produces on this surface, where the only refusals are 403 {"message":"Forbidden"} (unsigned AWS_IAM route) and 404 {"message":"Not Found"} (nothing matched), both JSON. Host:port, API ids, the marker, the origin string and the MaxAge number are per-run values, not requirements.

## Steps to Reproduce
Cause 1 (disabled default execute-api endpoint):
1. Boot the emulator from this checkout. Create a Lambda function whose handler returns 200 with a MARKER environment value as the body and prints a nonce header it received.
2. Create two HTTP APIs ('target' and 'twin'), each with an AWS_PROXY integration (PayloadFormatVersion '2.0') to that function, a route 'GET /hit' targeting the integration, and a $default stage with AutoDeploy=True; lambda AddPermission for apigateway.amazonaws.com on arn:<partition>:execute-api:<region>:<account>:<api-id>/*/* for both.
3. Confirm both APIs' execute-api URLs answer 200 with the marker on GET /hit.
4. UpdateApi(ApiId=<target>, DisableExecuteApiEndpoint=True); confirm GetApi reports true for the target and false for the twin.
5. GET the target URL repeatedly: expect a settled HTTP 404 with body {"message":"Not Found"}, no backend invocation (no nonce in the function's log group) and the twin still answering 200 with the marker in the same instant. Record the emulator's status, content-type and body.
6. UpdateApi(ApiId=<target>, DisableExecuteApiEndpoint=False) and GET again: expect a settled 200 with the marker and that request visible in the backend's log.
7. Re-replay apigatewayv2.deep-apigatewayv2-2-enforce-disable-execute-api-endpoint-404-and-restore.

Cause 2 (per-route throttling):
1. Create an HTTP API with one AWS_PROXY integration to a marker-echo Lambda and two routes, 'GET /limited' and 'GET /twin'.
2. CreateStage(StageName='$default', AutoDeploy=True, DefaultRouteSettings={ThrottlingRateLimit: 500.0, ThrottlingBurstLimit: 200}, RouteSettings={'GET /limited': {ThrottlingRateLimit: 1.0, ThrottlingBurstLimit: 1}}); confirm GetStage reports RouteSettings for 'GET /limited' only.
3. Send single requests to each route a few seconds apart: every one must answer 200 with the marker (a 1 rps route must not shed a caller pacing itself at well under 1 rps).
4. Fire an interleaved burst of ~24 requests at each route and record statuses and bodies: expect >= 1 429 carrying 'Too Many Requests' and no backend marker on /limited, 200 with the marker for the rest, and zero 429 on /twin.
5. DeleteRouteSettings(ApiId, StageName='$default', RouteKey='GET /limited'), confirm GetStage no longer reports it, and repeat the burst: nothing is shed.
6. Re-replay apigatewayv2.route-throttling-returns-429-while-twin-route-serves.

Cause 3 (Origin-bearing request on a CFN-deployed CORS API):
1. Deploy a CloudFormation stack with an AWS::ApiGatewayV2::Api (ProtocolType HTTP, CorsConfiguration AllowOrigins=['https://allowed.example.com'], AllowMethods=['GET'], MaxAge=300), an AWS::Lambda::Function returning a JSON marker body, an AWS_PROXY integration, a route 'GET /a' and a $default AutoDeploy stage.
2. GET <stage url>/a with NO Origin header — observe 200 with the Lambda's body.
3. Send the identical GET with header Origin: https://allowed.example.com — expect the same 200 and body plus access-control-allow-origin: https://allowed.example.com. Observe the emulator's status, content-type and body.
4. OPTIONS the same url with Origin + Access-Control-Request-Method: GET + Access-Control-Request-Headers: content-type — expect 204 with the origin echoed, GET in access-control-allow-methods and access-control-max-age: 300.
5. Update the stack removing CorsConfiguration and changing Description: expect a Modify with Replacement False on the Api resource, the same ApiId, GetApi without CorsConfiguration, the preflight now 404 with no access-control-* header, and GET /a still 200 with the marker and no CORS headers.
6. Re-replay apigatewayv2.deploy-apigatewayv2-1-http-api-cors-removed-in-place-then-api-replaced.

## Resolution

The ONE defect found behind all three causes: the invoke address the control plane PUBLISHED was
not an address this emulator answers on, so no request of any of these checks reached the data
plane at all. `Api$ApiEndpoint` was announced as `{api-id}.execute-api.{region}.localhost:{port}`
— a NAME whose resolution the caller's own resolver has to supply, which the replay harness
refuses outright ("PublishedUrlRejected: … its host is not one of the emulator's own hosts").
An HTTP api's data plane is now published as the ADDRESS the core's listener for that plane landed
on, with the api's own id behind the reserved data-plane path (`/_aws/execute-api/{api-id}`), so a
client dials the value as it stands and appending `/{stage}/{route-path}` to it is the whole invoke
path. The api's own documented `execute-api` host is still served at every front door, that socket
included, for a client that does resolve it. (The same fix landed on mainline from the sibling
issue 20261126T140544Z while this one was in review; that form is the one kept.)

The three causes' behaviours were already implemented in this tree and are now driven END TO END
at the published address, over a real socket, on an emulator STARTED from the built binary, by
`core/router/tests/all/apigatewayv2_published_endpoint_enforcements_on_a_started_emulator.rs`:

* cause 1 — an api whose `DisableExecuteApiEndpoint` is set answers `404`
  `{"message":"Not Found"}` on its published address with nothing of the integration in it, the
  twin api over the same function is served by it in the same instant, and clearing the member
  restores serving on the same address;
* cause 2 — a route the serving stage's `RouteSettings` meter sheds the excess of a burst with
  `429` `{"message":"Too Many Requests"}` without reaching the integration, the twin route under
  the stage's `DefaultRouteSettings` sheds nothing, the metered route is served again as its rate
  refills the bucket, and `DeleteRouteSettings` returns it to those defaults;
* cause 3 — an `Origin` decides nothing but the `Access-Control-*` decoration: the route answers
  the integration's body with an admitted origin, with a foreign one and with none, a preflight no
  route matches is the gateway's own `204` carrying the configured `MaxAge`, and after
  `DeleteCorsConfiguration` the preflight falls through to `404` `{"message":"Not Found"}` with no
  decoration while the GET keeps being served without one.

The three checks still end as `Check timed out after 60.0s` with no frame, no status and no value:
three different published forms (the documented name, the socket's address with the api in the
path, and the socket's address alone) produce byte-identical replay verdicts, while the same three
behaviours answer correctly at the published address on the started binary, so nothing in this tree
explains what the harness is waiting for. A later reader should treat the three checks as ungraded
rather than as passing.
