Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync.graphql-endpoint-cors-preflight-and-cross-origin-error-readability
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync.graphql-endpoint-cors-preflight-and-cross-origin-error-readability|run#31|AssertionError|assert code == 200 and (body.get("data") or {}).get("ping") == "pong", (
Issue-Commit: a9288303feb8dead80b5d9b52ed92320ffa70966

# AppSync GraphQL endpoint refuses a correctly keyed POST that carries an Origin header with 403 UnauthorizedException naming an APPSYNC_GRAPHQL_REMOTE_ORIGINS allowlist

> _Note: this issue was identified against the code base at commit `a9288303feb8dead80b5d9b52ed92320ffa70966`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGraphqlApi

## Description
REOPENS 20261114T140817Z-appsync-4-net-new-causes-the-graphql-endpoint.md: its fix left the call refused. That issue's cause 2 reported the CORS PREFLIGHT being refused 403 with the emulator-authored message 'cross-site origin'. The preflight now succeeds (the check gets past its OPTIONS assertions, including the Allow-Origin/Methods/Headers and Max-Age 172800 grants), but the cross-origin POST that follows it is refused instead, so the same check still cannot reach any of its later assertions. This is one cause.

AWS: the GraphQL data-plane POST is not modeled; the modeling context is Smithy (appsync) CreateGraphqlApi — POST /v1/apis, `authenticationType` (AuthenticationType) @required, response GraphqlApi$uris publishing the GRAPHQL endpoint. Recorded behavior only for the endpoint itself: a POST to GraphqlApi$uris.GRAPHQL that carries a valid `x-api-key` for the API is served exactly the same whether or not it carries an `Origin` request header — HTTP 200 with the normal {"data": {...}} envelope — and the response additionally carries `Access-Control-Allow-Origin: *` so a browser may read it. AppSync maintains NO origin allowlist: the Origin header never affects admission, and a 403 is never the answer to a request whose only distinguishing feature is that header. Correspondingly a refused request (a forged key) is 401 with body {"errors":[{"errorType":"UnauthorizedException", ...}]} that ALSO carries the Allow-Origin grant and exposes x-amzn-ErrorType, and a bare OPTIONS with no CORS request headers is not a preflight and is refused 404 UnknownOperationException with no CORS grant.
Emulator: the keyed cross-origin POST is answered HTTP 403 with body {"errors":[{"errorType":"UnauthorizedException","message":"This endpoint serves a page served from this machine. Name another site in APPSYNC_GRAPHQL_REMOTE_ORIGINS to serve a page from it."}]}. The status, the errorType and the entire sentence are emulator-authored: the sentence is a configuration instruction about an emulator environment variable (APPSYNC_GRAPHQL_REMOTE_ORIGINS) with no fixed or variable part corresponding to anything AWS emits, and the request it refuses is one AWS serves 200. Because the POST is refused, the check never reaches the 401-forged-key CORS assertions or the bare-OPTIONS 404 assertion, so those remain unverified behind this refusal.
Why: no browser-based client (Amplify, Apollo or urql in a page, the AppSync console) can call the emulator's GraphQL endpoint at all — the preflight now passes, the page issues the real POST, and the POST is rejected 403. Worse, the rejection is typed UnauthorizedException, so a client cannot tell 'my API key is wrong' from 'this request came from a page', and credential-refresh logic fires against a request whose credentials were valid. An emulator that admits only same-origin pages inverts AppSync's contract, where origin plays no part in admission and CORS grants are what make the error envelope readable.
Fix: the GraphQL endpoint handler's request-admission gate — remove the origin allowlist from the served path entirely (the Origin header must not be an input to admission), and attach the CORS grant (Access-Control-Allow-Origin: * plus Access-Control-Expose-Headers including x-amzn-RequestId and x-amzn-ErrorType) to served 200s and to refused 401s alike, while leaving a non-preflight OPTIONS refused 404 UnknownOperationException with no grant.
Traces: appsync.graphql-endpoint-cors-preflight-and-cross-origin-error-readability.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator built from this checkout. Check appsync.graphql-endpoint-cors-preflight-and-cross-origin-error-readability, listed under service heading `### appsync`, verdict [mismatch], not marked ALREADY FILED and not OWNED by any open issue.

  File "check_appsync_graphql_endpoint_cors_preflight_and_cross_origin_error_readability.py", line 113, in run
    assert code == 200 and (body.get("data") or {}).get("ping") == "pong"
  AssertionError: the cross-origin POST must be served, got 403 '{"errors":[{"errorType":"UnauthorizedException","message":"This endpoint serves a page served from this machine. Name another site in APPSYNC_GRAPHQL_REMOTE_ORIGINS to serve a page from it."}]}'

The same request WITHOUT an Origin header was served 200 with data.ping == 'pong' earlier in the same check (the poll at line ~113 that establishes the resolver answers), and the OPTIONS preflight assertions immediately before the failure all passed — so the only difference on the refused request is the Origin header.

Expected (AWS): HTTP 200, body {"data":{"ping":"pong"}}, header Access-Control-Allow-Origin: *.
Actual (emulator): HTTP 403, UnauthorizedException naming APPSYNC_GRAPHQL_REMOTE_ORIGINS.

Unverified behind this refusal (the check stops here): the 401 for a forged da2- key must carry Access-Control-Allow-Origin: * and expose x-amzn-ErrorType, and a bare OPTIONS must be 404 UnknownOperationException with no CORS grant.

Smithy cited: appsync CreateGraphqlApi (POST /v1/apis; authenticationType @required; GraphqlApi$uris). The data-plane POST, its {data,errors[{errorType,message}]} envelope and its CORS headers are not modeled — recorded behavior only.

## Steps to Reproduce
1. api = appsync.create_graphql_api(name=..., authenticationType="API_KEY")["graphqlApi"]; start_schema_creation with 'schema { query: Query } type Query { ping: String }' and wait for SUCCESS; create_data_source(type="NONE"); create_resolver for Query.ping (APPSYNC_JS, returning 'pong'); key = create_api_key(apiId=...)["apiKey"]["id"]; url = api["uris"]["GRAPHQL"].
2. POST {"query":"query { ping }"} to url with headers Content-Type: application/json and x-api-key: <key> and NO Origin -> served 200 {"data":{"ping":"pong"}} (control, passes).
3. OPTIONS url with Origin: https://sim2real.example, Access-Control-Request-Method: POST, Access-Control-Request-Headers: content-type,x-api-key -> 200, empty body, Access-Control-Allow-Origin '*' (passes today).
4. Repeat step 2 with the additional header Origin: https://sim2real.example.
   Expected: HTTP 200 {"data":{"ping":"pong"}} with Access-Control-Allow-Origin: *.
   Observed: HTTP 403 {"errors":[{"errorType":"UnauthorizedException","message":"This endpoint serves a page served from this machine. Name another site in APPSYNC_GRAPHQL_REMOTE_ORIGINS to serve a page from it."}]}.
5. Then the two assertions this refusal hides: POST with Origin and a forged key 'da2-' + 'x'*26 -> expect 401 whose body names errorType UnauthorizedException and whose headers carry Access-Control-Allow-Origin '*' and expose x-amzn-ErrorType; and OPTIONS url with no headers at all -> expect 404 errorType UnknownOperationException with NO Access-Control-Allow-Origin.
Re-replay appsync.graphql-endpoint-cors-preflight-and-cross-origin-error-readability to confirm.
