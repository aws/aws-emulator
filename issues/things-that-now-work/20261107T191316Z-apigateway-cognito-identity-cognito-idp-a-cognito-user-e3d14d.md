Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cognito-identity__cognito-idp.apigw-cognito-user-pool-client-app-client-id-validation-positive apigateway__cognito-identity__cognito-idp.apigw-cognito-authorizer-multiple-user-pools-audience-negative
Issue-Key: apigateway__cognito-identity__cognito-idp.apigw-cognito-user-pool-client-app-client-id-validation-positive|run#18|AssertionError|assert status == 200, f"expected 200 for a token issued to the configured client, got {status}: {body}"
Issue-Key: apigateway__cognito-identity__cognito-idp.apigw-cognito-authorizer-multiple-user-pools-audience-negative|run#16|AssertionError|assert control_status == 200, f"control (client_a token) should succeed, got {control_status}: {control_body}"
Issue-Commit: c3a6f169c346304d6b4c388bf8378b1c143b9d70

# apigateway__cognito-identity__cognito-idp: a COGNITO_USER_POOLS authorizer refuses a valid id token from the configured user pool and app client with 401 Unauthorized

> _Note: this issue was identified against the code base at commit `c3a6f169c346304d6b4c388bf8378b1c143b9d70`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke (execute-api data plane, COGNITO_USER_POOLS authorizer)

## Description
Cause 1 of 1: the COGNITO_USER_POOLS authorizer rejects the positive control — an id token minted by the very user pool named in the authorizer's providerARNs, issued to the app client the method is configured for.

AWS: a method with authorizationType=COGNITO_USER_POOLS and an authorizer whose providerARNs list the user pool validates the bearer token's signature, its `iss` against `https://cognito-idp.<region>.amazonaws.com/<userPoolId>`, its `token_use`, and (when configured) its `aud`/`client_id` against the API's app client. A token that satisfies those is ALLOWED and the request reaches the integration (200 here). Only a token from another pool, an unknown audience, an expired/garbage token or a missing Authorization header is refused — 401 with body `{"message":"Unauthorized"}` and `x-amzn-errortype: UnauthorizedException`.

Emulator: the positive leg is refused too. Every request carrying a valid id token for the configured pool and client is answered 401 `{"message":"Unauthorized"}`, so the authorizer never admits anyone. The 401 status, the error code and the message are the ones AWS uses for a REFUSAL — the defect is that they are served for a token AWS admits, i.e. the authorizer's allow decision is never reached. The sibling checks in this suite show why the comparison fails in the first place: this emulator mints `iss` as `https://cognito-idp.<region>.amazonaws.com/<userPoolId>` while the pool is reachable at the emulator's own endpoint, so an issuer/audience comparison that uses the emulator endpoint (or vice versa) can never match. Hypothesis: the authorizer compares the token's `iss` (or its `aud`/`client_id`) against a value derived from a different host or from the providerARN in a form the token never carries.

Why: no API protected by a Cognito user-pool authorizer can be called at all. Since the negative legs also return 401, a client cannot distinguish "the token is from the wrong pool/audience" (the behavior under test) from "the authorizer admits nobody", and any application that gates traffic on a user-pool authorizer is completely blocked.

Fix: the execute-api data-plane COGNITO_USER_POOLS authorizer — token validation and the issuer/audience comparison against the authorizer's providerARNs and the method's app client — not the control plane, which accepts the authorizer configuration.

Traces: apigateway__cognito-identity__cognito-idp.apigw-cognito-user-pool-client-app-client-id-validation-positive (the positive control), apigateway__cognito-identity__cognito-idp.apigw-cognito-authorizer-multiple-user-pools-audience-negative (its client_a positive control, which must succeed before the cross-audience negative means anything).

## Evidence
Expected: 200 from the integration for a token issued by the configured user pool to the configured app client.
Actual (apigw-cognito-user-pool-client-app-client-id-validation-positive): `AssertionError: expected 200 for a token issued to the configured client, got 401: {"message":"Unauthorized"}`.
Actual (apigw-cognito-authorizer-multiple-user-pools-audience-negative): `AssertionError: control (client_a token) should succeed, got 401: {"message":"Unauthorized"}`.
The 401/`UnauthorizedException`/`{"message":"Unauthorized"}` triple is correct AWS wording for a refusal; the divergence is that it is served where AWS serves the integration's 200. Corroborating issuer evidence from the same emulator (sibling checks in apigateway__cognito-idp__lambda): the id token's `claims.iss` is `https://cognito-idp.us-west-2.amazonaws.com/<userPoolId>` while the pool is served at `https://<host>:<port>/<userPoolId>`.

## Steps to Reproduce
1. CreateUserPool and CreateUserPoolClient; AdminCreateUser + AdminSetUserPassword; AdminInitiateAuth (ADMIN_USER_PASSWORD_AUTH) to obtain an IdToken.
2. CreateRestApi + CreateResource + CreateAuthorizer(type="COGNITO_USER_POOLS", providerARNs=[<pool arn>], identitySource="method.request.header.Authorization").
3. PutMethod(authorizationType="COGNITO_USER_POOLS", authorizerId=<id>), wire a MOCK or Lambda integration, CreateDeployment(stageName=s).
4. GET the stage URL with `Authorization: <IdToken>` -> 401 `{"message":"Unauthorized"}` instead of the integration's 200.
5. Repeat with a token minted by a second pool / a different app client: also 401, so the negative is indistinguishable from the positive.
