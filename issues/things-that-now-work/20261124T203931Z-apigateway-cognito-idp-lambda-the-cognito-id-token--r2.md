Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cognito-idp__lambda.apigw-cognito-authorizer-claims-passthrough-to-lambda apigateway__cognito-idp__lambda.apigw-cognito-authorizer-claims-reach-integration
Issue-Key: apigateway__cognito-idp__lambda.apigw-cognito-authorizer-claims-passthrough-to-lambda|run#24|AssertionError|assert not iss_wrong, f"claims.iss {claims['iss']!r} is not the pool's issuer: {iss_wrong}"
Issue-Key: apigateway__cognito-idp__lambda.apigw-cognito-authorizer-claims-reach-integration|run#19|AssertionError|assert not iss_wrong, (
Issue-Commit: b396de6621b98a4950294fcbb23a4743977dd096

# apigateway__cognito-idp__lambda: the Cognito ID token's iss claim is published on cognito-idp.<region>.localhost:<port>, a host the emulator does not serve, so the issuer surfaced in requestContext.authorizer.claims is still unreachable

> _Note: this issue was identified against the code base at commit `b396de6621b98a4950294fcbb23a4743977dd096`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AdminInitiateAuth

## Description
Cause 1 of 1: the issuer (iss) claim of every ID token a user pool mints — and therefore the iss the API Gateway COGNITO_USER_POOLS authorizer copies into event.requestContext.authorizer.claims — names a host that this emulator does not answer on. The previously CLOSED issue 20261114T110649Z-apigateway-cognito-idp-lambda-the-cognito-id-token.md carried exactly these two checks and described the same cause with the earlier spelling 'https://cognito-idp.<region>.amazonaws.com/<poolId>'; its fix only changed the host suffix (now '<...>.localhost:<port>'), so the announced issuer host is still not one the emulator serves and both checks fail at the same assertion. The resolved issue does not cover this failure, so it is re-filed with the checks' keys; fold it into that work if it is still open.

AWS: recorded behavior only — the JWT body is not Smithy-modelled. The token comes from AdminInitiateAuth (Smithy: AdminInitiateAuth -> AuthenticationResult (AuthenticationResultType), IdToken), and apigatewayv2 JWTConfiguration$Issuer documents the form: 'The base domain of the identity provider that issues JSON Web Tokens. For example, an Amazon Cognito user pool has the following format: https://cognito-idp.{region}.amazonaws.com/{userPoolId}'. The requirement is the FORM plus REACHABILITY: scheme https; a host whose first two labels are the literal 'cognito-idp' and the pool's region followed by at least one further domain label; a path of exactly one segment equal to the pool id (<region>_<suffix>); no query, no fragment. In an emulated deployment the host must additionally be an address the emulator itself answers on (the endpoint a caller reaches cognito-idp on, or an own per-resource listener host), because a verifier dereferences iss for /.well-known/openid-configuration and /.well-known/jwks.json. The host, port and pool id of any one run are environment/state values, not part of the requirement.

Emulator: the minted ID token carries iss = 'https://cognito-idp.<region>.localhost:<port>/<poolId>' (observed 'https://cognito-idp.us-west-2.localhost:34493/us-west-2_a891898db' and 'https://cognito-idp.us-west-2.localhost:39955/us-west-2_7f87badca'; host/port/pool id are per-run values). The same value reaches the Lambda integration verbatim as claims.iss, and the harness refuses it: "the emulator published <iss>: its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener". The 'cognito-idp.<region>' labels and the /<poolId> path are now correct; the defect is that the announced authority is a synthesized name the emulator neither serves nor announces as one of its own hosts.

Why: every OIDC/JWT verifier performs the issuer check and derives the JWKS URI from iss. A client, or a Lambda handler reading event.requestContext.authorizer.claims.iss, that tries to validate the token — or merely to tell two providers apart by issuer — cannot dereference the announced issuer at all, so no token this emulator issues can be verified downstream even though the token itself is well-formed.

Fix: the cognito-idp per-pool issuer computation shared by the token-minting path (InitiateAuth/AdminInitiateAuth/RespondToAuthChallenge) and the OpenID discovery + JWKS serving path: the stored/rendered issuer URL must resolve to an address the emulator announces and serves (the dialled cognito-idp endpoint, or a per-resource listener host registered as the emulator's own) while keeping the 'cognito-idp.<region>' labels and the /<poolId> path segment. The API Gateway authorizer's claims projection is correct and needs no change — it passes the stored value through.

Traces: apigateway__cognito-idp__lambda.apigw-cognito-authorizer-claims-passthrough-to-lambda (assertion at check line 62), apigateway__cognito-idp__lambda.apigw-cognito-authorizer-claims-reach-integration (assertion at check line 119).

## Evidence
Expected (AWS): claims.iss is the pool's issuer URL — https scheme, host labels beginning 'cognito-idp' + <region> plus a further domain label, path exactly /<userPoolId> — served at an address the emulator answers on.

Actual (emulator), check apigw-cognito-authorizer-claims-passthrough-to-lambda:
AssertionError: claims.iss 'https://cognito-idp.us-west-2.localhost:34493/us-west-2_a891898db' is not the pool's issuer: the emulator published https://cognito-idp.us-west-2.localhost:34493/us-west-2_a891898db: its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:34493/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener

Actual (emulator), check apigw-cognito-authorizer-claims-reach-integration:
AssertionError: claims.iss 'https://cognito-idp.us-west-2.localhost:39955/us-west-2_7f87badca' is not the pool's issuer: the emulator published https://cognito-idp.us-west-2.localhost:39955/us-west-2_7f87badca: its host is not one of the emulator's own hosts; ...

Every other claim asserted by these checks (token_use='id', cognito:username, email, aud=<client id>, sub=<the sub from AdminCreateUser>) matched, and the request reached the integration with HTTP 200 — only the issuer authority diverges. Host, port and pool id above are per-run values; the invariant that fails is 'the announced issuer host is one the emulator serves'.

## Steps to Reproduce
1. CreateUserPool + CreateUserPoolClient (ALLOW_ADMIN_USER_PASSWORD_AUTH); AdminCreateUser with email/email_verified, then AdminSetUserPassword(Permanent=true).
2. AdminInitiateAuth (ADMIN_USER_PASSWORD_AUTH) and take AuthenticationResult.IdToken; base64url-decode the payload and read iss — it is https://cognito-idp.<region>.localhost:<port>/<poolId>, an authority the emulator does not serve or announce.
3. Equivalently, end to end: create a REST API with a COGNITO_USER_POOLS authorizer over that pool (providerARNs=[pool arn], identitySource=method.request.header.Authorization), an AWS_PROXY Lambda integration that echoes event.requestContext.authorizer.claims, deploy a stage, and GET the method with 'Authorization: <IdToken>'. The response is 200 and claims.iss carries the same unreachable authority.
4. Attempt to GET <iss>/.well-known/openid-configuration as a client of the emulator: the host is not one the emulator serves.
