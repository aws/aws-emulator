Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__cognito-idp__lambda.http-api-jwt-authorization-scope-mismatch-not-delivered apigatewayv2__cognito-idp__lambda.http-api-jwt-authorizer-rejects-foreign-issuer-and-audience apigatewayv2__cognito-idp__lambda.jwt-authorizer-claims-reach-the-lambda-event apigatewayv2__cognito-idp__lambda.jwt-authorizer-expired-token-rejected-never-reaches-lambda
Issue-Key: apigatewayv2__cognito-idp__lambda.http-api-jwt-authorization-scope-mismatch-not-delivered|_v2_invoke_url#18|PublishedUrlRejected|return f"{ctx.stage_url(api_endpoint, stage or '$default')}/{path.lstrip('/')}"
Issue-Key: apigatewayv2__cognito-idp__lambda.http-api-jwt-authorizer-rejects-foreign-issuer-and-audience|_v2_invoke_url#18|PublishedUrlRejected|return f"{ctx.stage_url(api_endpoint, stage or '$default')}/{path.lstrip('/')}"
Issue-Key: apigatewayv2__cognito-idp__lambda.jwt-authorizer-claims-reach-the-lambda-event|_v2_invoke_url#17|PublishedUrlRejected|return f"{ctx.stage_url(api_endpoint, stage or '$default')}/{path.lstrip('/')}"
Issue-Key: apigatewayv2__cognito-idp__lambda.jwt-authorizer-expired-token-rejected-never-reaches-lambda|_v2_invoke_url#16|PublishedUrlRejected|return f"{ctx.stage_url(api_endpoint, stage or '$default')}/{path.lstrip('/')}"
Issue-Commit: ec22e48eafa5e4290d9bdfafda3d7e5b1d68564a

# apigatewayv2__cognito-idp__lambda: no JWT-authorizer behaviour can be observed — the published ApiEndpoint is an unserved '<api-id>.execute-api.<region>.localhost:<port>' host (4 checks)

> _Note: this issue was identified against the code base at commit `ec22e48eafa5e4290d9bdfafda3d7e5b1d68564a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApi

## Description
Cause 1 of 1: the invoke address published for the HTTP API is not an address the emulator answers on, so no request can reach a JWT-authorized route.

AWS: Smithy (apigatewayv2) documents Api$ApiEndpoint as "The URI of the API, of the form {api-id}.execute-api.{region}.amazonaws.com. The stage name is typically appended to this URI to form a complete path to a deployed API stage." On AWS that host serves the API: a token missing a required scope or carrying a foreign issuer/audience, or an expired token, is refused by the gateway before the integration runs (403 with body {"message":"Forbidden"} for an authorization failure, 401 {"message":"Unauthorized"} for an unusable token), and an accepted token's claims arrive in the integration event under requestContext.authorizer.jwt.claims/scopes.

Emulator: it publishes 'https://<api-id>.execute-api.<region>.localhost:<ephemeral-port>' while listening only on 127.0.0.1:<core-port>. The dial is refused before any request is sent:
  simparity.published.PublishedUrlRejected: endpoint_url was given https://<api-id>.execute-api.<region>.localhost:<port> (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<core-port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener
The api-id, region label and ports are per-run values; the requirement is that the published address is one this emulator answers on.

Why: a client holding a Cognito token has no reachable address for the API, so neither the refusals (scope mismatch, foreign issuer/audience, expiry) nor the claims propagation into the Lambda event can be exercised — the authorizer contract is unobservable.

Fix: the ApiEndpoint publication in the apigatewayv2 Api response serializers must announce an address the emulator serves, and the execute-api front door must route that host so the JWT authorizer on the route is reached. Context: fixed issues 20261109T124858Z-apigatewayv2-cognito-idp-lambda-an-http-api-s.md and 20261114T171242Z-apigatewayv2-cognito-idp-lambda-reopens-an-http-api-s-invoke-address-is.md are the same family of unreachable invoke address, now with a '.localhost' host instead of an amazonaws.com one.

Traces: the 4 failing checks in trace_ids.

## Evidence
Expected: GET '<ApiEndpoint>/<route path>' with a Cognito access/ID token — accepted tokens are served 200 and the integration event carries requestContext.authorizer.jwt.claims; a scope mismatch or foreign issuer/audience is refused 403 {"message":"Forbidden"} and an expired token 401 {"message":"Unauthorized"}, with the Lambda never invoked.

Actual (all 4 checks under '### apigatewayv2__cognito-idp__lambda'): no request is delivered. Each fails with
  simparity.published.PublishedUrlRejected: endpoint_url was given https://mp67t4dbd4.execute-api.us-west-2.localhost:44861 (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:33377/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener
(host/port values differ per check and per run.)

## Steps to Reproduce
1. Create a Cognito user pool + client; CreateApi(ProtocolType='HTTP') with a JWT authorizer (Issuer=the pool, Audience=the client), a route with AuthorizationScopes and an AWS_PROXY Lambda integration, plus a stage.
2. Mint a token (correct scope / wrong scope / foreign issuer / expired).
3. Read ApiEndpoint and GET '<ApiEndpoint>/<route path>' with Authorization: Bearer <token>.
4. AWS: 200 with claims in the event, or the modelled 403/401. Emulator: the published host '<api-id>.execute-api.<region>.localhost:<port>' is not served, so nothing is delivered.
