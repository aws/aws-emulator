Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync__cognito-idp.deep-appsync-2-appsync-authz-oidc-iat-ttl-rejects-aged-token
Issue-Key: appsync__cognito-idp.deep-appsync-2-appsync-authz-oidc-iat-ttl-rejects-aged-token|_poll_until#10|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Commit: 36cbc41e18f8f7f03eaee998bf950ce282c69b6c

# appsync__cognito-idp: an APPSYNC_JS resolver over a NONE data source resolves to null — the response function's value is never returned, so no OIDC authorization leg can be reached (1 cause)

> _Note: this issue was identified against the code base at commit `36cbc41e18f8f7f03eaee998bf950ce282c69b6c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PostGraphQL (AppSync GraphQL data plane) / CreateResolver

## Description
Cause 1 of 1: a UNIT resolver with runtime APPSYNC_JS and `code` attached to a type=NONE (local) data source is answered HTTP 200 with the field null and no error, instead of the value its response function returns.
AWS: Smithy — Resolver$code is "The resolver code that contains the request and response functions. When code is used, the runtime is required. The runtime value must be APPSYNC_JS" (Code @length 1..32768), and Resolver$kind UNIT runs the field against a single data source. For a NONE data source there is no external call: AppSync runs the request function, passes its payload straight through, then runs the response function and returns its value as data.<field>. For the check's pong resolver the response is HTTP 200 with body exactly {"data": {"ping": "pong"}} and no "errors" member. This field exists only as the authorization probe for the API: an OPENID_CONNECT API with openIDConnectConfig.iatTTL serves it for a token whose iat is inside iatTTL and refuses it with HTTP 401 UnauthorizedException once the token is older than iatTTL (recorded behavior for the authorization legs; the data-plane contract is not in the control-plane model).
Emulator: the request is admitted (HTTP 200) but the body is {"data": {"ping": null}} — no "errors" member, no diagnosis, just a null field; this is served repeatedly for 240 s of polling with a freshly minted token each time, on both the iatTTL API and its twin without iatTTL. Because the warm-up leg demands the documented body, the check never gets as far as presenting an aged token, so the iatTTL refusal (401 / UnauthorizedException) is not exercised at all.
Why: a client gets a successful-looking 200 whose field is empty, with nothing saying why — every APPSYNC_JS resolver backed by a NONE data source (the standard pattern for local/pass-through fields, health probes and pipeline stitching) silently returns null, and any authorization behaviour that can only be observed through such a field becomes unobservable.
Fix: the GraphQL data-plane resolver-execution path for runtime APPSYNC_JS over a DataSource of type NONE: run the request function, carry its payload through the (absent) data-source call, run the response function and populate data.<field> from its return value (surfacing a thrown/util.error as an `errors` entry instead of a bare null).
Traces: appsync__cognito-idp.deep-appsync-2-appsync-authz-oidc-iat-ttl-rejects-aged-token. Open issue 20261110T155328Z-appsync-cognito-idp-an-openid-connect-graphql-api.md describes the earlier state of this same check (the published uris.GRAPHQL naming an unserved real-AWS host); the endpoint is now reached and answers 200, so this execution gap is the next, distinct cause.

## Evidence
Verbatim from the replay of appsync__cognito-idp.deep-appsync-2-appsync-authz-oidc-iat-ttl-rejects-aged-token:
  AssertionError: the ttl OIDC api to accept a freshly minted token not met within 240.0s. Last result: (200, {'data': {'ping': None}})
The polled predicate is `res[0] == 200 and res[1] == {"data": {"ping": "pong"}}`.
Expected (AWS): HTTP 200 with body {"data": {"ping": "pong"}} (no "errors" member) for a token whose iat is within openIDConnectConfig.iatTTL, and HTTP 401 with an UnauthorizedException error once the same unexpired token is older than iatTTL.
Actual (emulator): HTTP 200 with body {"data": {"ping": null}}, for every attempt over 240 s, on both the iatTTL API and the twin without iatTTL. The resolver was created with CreateResolver(kind=UNIT, dataSourceName=<type NONE data source>, runtime=APPSYNC_JS, code=<request/response functions returning "pong">) and GetGraphqlApi confirmed openIDConnectConfig.iatTTL was retained, so both the API and the resolver were accepted by the control plane.

## Steps to Reproduce
1. CreateUserPool + an admin-auth app client + one confirmed user (the OIDC issuer/clientId for the API).
2. CreateGraphqlApi with authenticationType=OPENID_CONNECT and openIDConnectConfig={issuer, clientId, iatTTL: 8000}; StartSchemaCreation with `type Query { ping: String }` and wait for SUCCESS.
3. CreateDataSource(type="NONE") and CreateResolver(typeName="Query", fieldName="ping", kind="UNIT", dataSourceName=<that source>, runtime=APPSYNC_JS, code=<request function returning {} and response function returning "pong">).
4. POST {"query": "query P { ping }"} to the API's uris.GRAPHQL with a freshly minted ID token in Authorization.
Observed: HTTP 200, {"data": {"ping": null}}. Expected: HTTP 200, {"data": {"ping": "pong"}}.
5. Then wait until the token's iat is older than iatTTL and repeat: expect HTTP 401 UnauthorizedException (unreachable today because step 4 never yields the documented body).
