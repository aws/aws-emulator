Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__cognito-idp__lambda.jwt-authorizer-claims-reach-the-lambda-event apigatewayv2__cognito-idp__lambda.http-api-jwt-authorizer-rejects-foreign-issuer-and-audience
Issue-Key: apigatewayv2__cognito-idp__lambda.jwt-authorizer-claims-reach-the-lambda-event|run#23|AssertionError|assert claims.get(key) == expected.get(key), (
Issue-Key: apigatewayv2__cognito-idp__lambda.http-api-jwt-authorizer-rejects-foreign-issuer-and-audience|_poll_until#26|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Commit: 63b1f768cca526e685e8991a6e4ba2a43400c274

# apigatewayv2/cognito-idp/lambda: HTTP API JWT authorizer passes an empty jwt.claims map to the integration, and the request after a refused one never reaches the integration (2 causes)

> _Note: this issue was identified against the code base at commit `63b1f768cca526e685e8991a6e4ba2a43400c274`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAuthorizer

## Description
Two net-new root causes in the apigatewayv2 JWT-authorizer path, both grounded in failing checks the replay report marked neither ALREADY FILED nor OWNED.

Cause 1 of 2 — requestContext.authorizer.jwt.claims is an empty map for a request bearing a valid, pool-issued token the authorizer admitted.
AWS: an HTTP API route protected by a JWT authorizer (Smithy apigatewayv2 CreateAuthorizer with AuthorizerType=JWT and a JWTConfiguration{Issuer, Audience}) validates the bearer token and then hands the integration a payload-2.0 event in which requestContext.authorizer.jwt.claims carries the token's own verified claims — sub, client_id, token_use, scope, iss, exp, iat, jti — and requestContext.authorizer.jwt.scopes carries its scopes. A handler reads the caller identity from exactly there.
Emulator: the handler observes requestContext.authorizer.jwt.claims == {} — no 'sub', no 'client_id', nothing — while the request is still routed and served, so the authorizer admitted the token but contributed no claims.
Why: every HTTP API backend that authorizes or personalizes on the JWT claims sees an anonymous request; worse, it sees a request that LOOKS admitted, so a client cannot distinguish 'authorized caller X' from 'claims unavailable' and will either fail closed on valid traffic or treat an unidentified caller as authorized.
Fix: the apigatewayv2 JWT authorizer evaluation and the payload-2.0 event shaper for the HTTP API data plane — populate requestContext.authorizer.jwt.claims/scopes from the verified token.
Failing check: apigatewayv2__cognito-idp__lambda.jwt-authorizer-claims-reach-the-lambda-event.

Cause 2 of 2 — after the authorizer refuses a token with a foreign issuer/audience, the following control request never reaches the integration at all.
AWS: a JWT authorizer refusal is scoped to the refused request: the route is unaffected, and the next request carrying a valid token for the configured issuer and audience is forwarded to the integration, which runs and emits its log line. The refusal itself must be the documented 401 and must not invoke the backend.
Emulator: the control request sent after the refused one produces no integration log line within the run budget, so the integration was never invoked — the refusal (or the authorizer's state after it) wedges the route rather than being confined to the one request.
Why: a client cannot verify that a negative authorization outcome is per-request; one bad token appears to take the route out of service, and the negative-case contract ('refused request not forwarded, next good request forwarded') cannot be observed.
Fix: the same apigatewayv2 JWT authorizer evaluation path — the refusal must be per-request, leaving the route and its integration dispatch intact for subsequent requests.
Failing check: apigatewayv2__cognito-idp__lambda.http-api-jwt-authorizer-rejects-foreign-issuer-and-audience.

## Evidence
Cause 1: AssertionError: requestContext.authorizer.jwt.claims['sub'] must equal the token's own 'sub' claim ('<token sub>'), got None — full claims: {}. The 'sub' value is a per-run identifier that fills the template; the requirement is that the claim set is the token's verified claims, not that it is empty.
Cause 2: TimeoutError: the integration's log line for the control request sent after the refused one: the run budget ended first; not graded — raised from the check's _assert_backend_not_invoked/_poll_until after waiting for the integration's own 'HIT <marker>' log line. Expected the control request to be forwarded and the integration to log; observed no integration invocation at all.
Neither check carried an 'ALREADY FILED' or 'OWNED by' marker.

## Steps to Reproduce
Common setup: CreateUserPool + CreateUserPoolClient, obtain an access token; create an HTTP API (apigatewayv2 CreateApi ProtocolType=HTTP), a Lambda-proxy integration with payload format 2.0 whose handler prints its marker and echoes requestContext.authorizer.jwt.claims, a route, and CreateAuthorizer(AuthorizerType='JWT', JWTConfiguration={'Issuer': <pool issuer url>, 'Audience': [<client id>]}) attached to the route; deploy the default stage.
Cause 1: GET the route with Authorization: Bearer <valid pool token> -> expect 200 and a body whose claims contain the token's own sub/client_id, observe claims {}.
Cause 2: GET the route with a token minted for a foreign issuer/audience -> expect a 401 and no integration invocation; then GET again with the valid token -> expect the integration to run and log its marker line, observe no integration log line for the control request.
