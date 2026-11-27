Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cognito-identity__cognito-idp.apigw-cognito-authorizer-multiple-user-pools-audience-negative apigateway__cognito-identity__cognito-idp.apigw-cognito-user-pool-client-app-client-id-validation-positive
Issue-Key: apigateway__cognito-identity__cognito-idp.apigw-cognito-authorizer-multiple-user-pools-audience-negative|run#14|PublishedUrlRejected|url = f"{ctx.stage_url(api_endpoint, '$default')}/test"
Issue-Key: apigateway__cognito-identity__cognito-idp.apigw-cognito-user-pool-client-app-client-id-validation-positive|run#16|PublishedUrlRejected|url = f"{ctx.stage_url(api_endpoint, '$default')}/test"
Issue-Commit: ec22e48eafa5e4290d9bdfafda3d7e5b1d68564a

# REOPENS: both Cognito JWT-audience checks now fail before any request is sent — the HTTP API's published ApiEndpoint is an <apiId>.execute-api.<region>.localhost:<port> host the emulator is not reached on

> _Note: this issue was identified against the code base at commit `ec22e48eafa5e4290d9bdfafda3d7e5b1d68564a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApi

## Description
REOPENS 20261124T203931Z-apigateway-cognito-identity-cognito-idp-an-http-api--r2.md: its fix left the call refused. That resolved issue carries exactly these two checks' replay keys and claimed the HTTP API behind a Cognito JWT authorizer would serve them; on replay neither check gets as far as sending a request — the published invoke address is refused at egress — so the calls are still refused and the behaviour is re-filed with their keys.

Cause 1 of 1: apigatewayv2 publishes the HTTP API's invoke address as a synthesized name in the `.localhost` zone that no client can resolve to this emulator, so a client that follows the published endpoint cannot invoke the JWT-protected route at all. Failing check ids: apigateway__cognito-identity__cognito-idp.apigw-cognito-authorizer-multiple-user-pools-audience-negative, apigateway__cognito-identity__cognito-idp.apigw-cognito-user-pool-client-app-client-id-validation-positive.

AWS: Smithy apigatewayv2 Api$ApiEndpoint (returned by CreateApi and GetApi, target __string) is documented as "The URI of the API, of the form {api-id}.execute-api.{region}.amazonaws.com. The stage name is typically appended to this URI to form a complete path to a deployed API stage." — it is the address a client dials, and AWS serves that name. For an emulator the requirement is reachability rather than the literal AWS name: recorded behavior only — a value published as an invoke address must be an address the emulator itself answers on (http(s)://<own host>:<port>/<path>), because a client follows it verbatim. Behind a reachable endpoint, Smithy JWTConfiguration$Audience ("A list of the intended recipients of the JWT. A valid JWT must provide an aud that matches at least one entry in this list") and JWTConfiguration$Issuer govern the answer: a GET carrying `Authorization: Bearer <ID token whose aud is a listed client and whose iss is the configured issuer>` is authorized and reaches the AWS_PROXY integration (200 with the function's body), and the same request with a token issued to an unlisted client of another pool is refused HTTP 401 with the integration never invoked.

Emulator: CreateApi answers ApiEndpoint as `https://{api-id}.execute-api.{region}.localhost:{port}` — the api-id, region and port are per-run values; the HOST FORM (a synthesized three-label name in the RFC 6761 `.localhost` zone) is the defect, since nothing resolves that name to this process and it is not one of the emulator's own hosts. Both checks are refused at egress before any HTTP request is made: `simparity.published.PublishedUrlRejected: endpoint_url was given https://<apiId>.execute-api.<region>.localhost:<port> (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener`. No status, error code or body is produced, because nothing is sent. (Previously these two checks burned their whole budget polling the same endpoint; now the refusal is immediate, so the endpoint is still unusable.)

Why: a Cognito user pool fronting an HTTP API through a JWT authorizer is the standard pattern here. A client can provision the pool, the clients, the authorizer, the route and the $default stage successfully and then find the published endpoint undialable, so the allow path (aud matches a configured client) is unusable and the refusal path (a token from another pool/an unlisted audience answered 401) has no enforcement to observe at all.

Fix: the apigatewayv2 ApiEndpoint publication — the authority builder behind CreateApi/GetApi/GetApis must announce the address the invoke-plane listener actually landed on (its own host and port, path-style if that is how the front door serves the plane) rather than folding the api id, the region and an `execute-api` label into a `.localhost` name; the execute-api front door must then answer that published address for the api's stages and routes so the JWT authorizer decision becomes observable.

Traces: apigateway__cognito-identity__cognito-idp.apigw-cognito-authorizer-multiple-user-pools-audience-negative, apigateway__cognito-identity__cognito-idp.apigw-cognito-user-pool-client-app-client-id-validation-positive. Code sites: services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/ops/create_api.rs lines 486-500 and services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/state/invoke_plane_transport.rs lines 102-139 (`announced_endpoint` / `announced_authority`).

## Evidence
Replay (apigateway__cognito-identity__cognito-idp.apigw-cognito-authorizer-multiple-user-pools-audience-negative, verdict: mismatch):
```
File "context.py", line 616, in endpoint_route
    return self._published_target(url, "endpoint_url"), {}
simparity.published.PublishedUrlRejected: endpoint_url was given https://mp67t4dbd4.execute-api.us-west-2.localhost:42759 (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:42953/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener
```
Replay (apigateway__cognito-identity__cognito-idp.apigw-cognito-user-pool-client-app-client-id-validation-positive, verdict: mismatch):
```
simparity.published.PublishedUrlRejected: endpoint_url was given https://mp67t4dbd4.execute-api.us-west-2.localhost:43231 (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:34563/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener
```
Both targets come from the emulator's own response — the checks build `url = f"{ctx.stage_url(api_endpoint, '$default')}/test"` where api_endpoint is `api["ApiEndpoint"]` from CreateApi.

Expected: ApiEndpoint names an emulator-served address, so GET <ApiEndpoint>/test with a Bearer ID token issued to a client listed in JwtConfiguration.Audience is answered 200 with the integration's body, and the same request with a token from an unlisted client/another pool is answered 401 with no invocation.
Actual: neither request is ever sent; the published host is refused. (The id `mp67t4dbd4`, the region and the ports are per-run values; the host FORM is the requirement.)

## Steps to Reproduce
1. cognito-idp: CreateUserPool (two pools for the audience-negative leg), CreateUserPoolClient per pool, AdminCreateUser + AdminSetUserPassword(Permanent=true), AdminInitiateAuth to mint ID tokens.
2. lambda: CreateFunction returning {"statusCode":200,"body":"ok"}; AddPermission for apigateway.amazonaws.com.
3. apigatewayv2 CreateApi(Name=<n>, ProtocolType="HTTP"); CreateIntegration(AWS_PROXY, PayloadFormatVersion="2.0"); CreateAuthorizer(AuthorizerType="JWT", IdentitySource=["$request.header.Authorization"], JwtConfiguration={Issuer:<pool issuer>, Audience:[<client_a id>]}); CreateRoute(RouteKey="GET /test", AuthorizationType="JWT", AuthorizerId=<id>, Target="integrations/<id>"); CreateStage(StageName="$default", AutoDeploy=true).
4. Read ApiEndpoint from the CreateApi response: observe `https://<apiId>.execute-api.<region>.localhost:<port>`.
5. GET <ApiEndpoint>/test with `Authorization: Bearer <token for client_a>` (expected 200) and then with a token from the unlisted client/other pool (expected 401). Actual: the published address is not one the emulator is reached on, so neither request is made.
6. Equivalent: re-replay checks apigateway__cognito-identity__cognito-idp.apigw-cognito-user-pool-client-app-client-id-validation-positive and .apigw-cognito-authorizer-multiple-user-pools-audience-negative.
