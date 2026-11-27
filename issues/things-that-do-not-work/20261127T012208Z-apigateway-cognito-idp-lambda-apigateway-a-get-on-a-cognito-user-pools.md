Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cognito-idp__lambda.apigw-cognito-authorizer-missing-token-not-delivered
Issue-Key: apigateway__cognito-idp__lambda.apigw-cognito-authorizer-missing-token-not-delivered|_stays_absent#20|Timeout|time.sleep(interval_s)
Issue-Commit: 1ade082ac7b155d62ba7589cf4124913a2bd40fe

# apigateway: a GET on a COGNITO_USER_POOLS-authorized REST method never reaches a 401/403/200 verdict, so the authorizer's negative-delivery behaviour cannot be observed

> _Note: this issue was identified against the code base at commit `1ade082ac7b155d62ba7589cf4124913a2bd40fe`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAuthorizer

## Description
One net-new root cause, reached through a deployed REST API stage whose method is authorized by a COGNITO_USER_POOLS authorizer.

Cause 1 of 1: a request to a deployed method with authorizationType COGNITO_USER_POOLS never settles on any of the statuses AWS can answer for it, so the check is killed at the suite's 60-second budget with no verdict. Failing check id: apigateway__cognito-idp__lambda.apigw-cognito-authorizer-missing-token-not-delivered.

AWS:      recorded behavior only for the data plane (the Smithy model describes the control plane: com.amazonaws.apigateway#CreateAuthorizer takes `type` (AuthorizerType enum, which includes COGNITO_USER_POOLS), `providerARNs` and `identitySource`, and PutMethod takes `authorizationType`; none of them models the invoke-time response). The recording shows, on a GET /secure deployed to a stage, with authorizationType COGNITO_USER_POOLS, providerARNs = [the user pool ARN], identitySource = method.request.header.Authorization, and an AWS_PROXY integration to a Lambda:
  - a request with NO Authorization header: HTTP 401 with API Gateway's Unauthorized envelope (the body contains the word "Unauthorized"), and the backend is NOT invoked;
  - a request whose Authorization header is a token-shaped string that is not a pool token: HTTP 401, and the backend is NOT invoked;
  - a request carrying a real IdToken from the pool: HTTP 200 with the integration's body, and the backend IS invoked (its CloudWatch log line appears).
  Every one of those three responses is a terminal status the first request through a freshly deployed stage reaches within the check's budget on AWS.

Emulator: no verdict at all. The first observation — the unauthenticated GET, polled until the status is one of 200/401/403 — never reaches any of those three statuses, and the check is terminated: `Error: Check timed out after 60.0s`. Hypothesis (the suite reports only the timeout, so this is not stated by the recording): the COGNITO_USER_POOLS method answers a status outside {200, 401, 403} — e.g. a 404 because the authorized method/stage is not routed, or a 500/502 because the authorizer type is not handled at invoke time — and the poll therefore never accepts it. Which of the three it is, and whether the backend is reached, cannot be read off this run; the observable defect is that the request never settles on a status AWS answers for this configuration.

Why:      this is a security boundary. A client relying on AWS's behavior expects an unauthenticated or bogus-token request to a COGNITO_USER_POOLS method to be refused 401 AT THE GATEWAY with the Unauthorized envelope and the backend never invoked, and a valid pool IdToken to pass with 200. In the emulator the method answers neither the refusal nor the success, so a private API fronted by a Cognito user-pool authorizer can be neither used nor verified: an integration test for "unauthenticated callers are refused" hangs instead of passing, and nothing establishes that the backend was not reached.

Fix:      the REST API invoke path's authorizer dispatch for authorizationType COGNITO_USER_POOLS — resolve the method's authorizer, read the configured identitySource header, verify the bearer token against the pool named in providerARNs, and answer 401 with API Gateway's Unauthorized body without invoking the integration when the header is absent or the token does not verify, or invoke the AWS_PROXY integration and return 200 when it does. The control-plane seam (CreateAuthorizer / PutMethod storing type, providerARNs and identitySource) must carry those fields through to that dispatch.

Note: the open issue 20261111T130919Z-apigateway-cognito-identity-cognito-idp-testinvokeauthorizer-ignores-authorizer.md concerns TestInvokeAuthorizer and Authorizer$authorizerCredentials, not the invoke-time dispatch of a COGNITO_USER_POOLS method, and does not carry this check; the suite marks this check neither ALREADY FILED nor OWNED.

Traces:   apigateway__cognito-idp__lambda.apigw-cognito-authorizer-missing-token-not-delivered.

## Evidence
The check is listed under the suite report's `### apigateway__cognito-idp__lambda` heading with verdict [mismatch]. API ids, pool ids, function names, markers and the generated username/password below are per-run values and are never part of the requirement.

Check apigateway__cognito-idp__lambda.apigw-cognito-authorizer-missing-token-not-delivered [mismatch]
  Setup performed before the first observation, all accepted: CreateUserPool + AdminCreateUser + AdminSetUserPassword + AdminInitiateAuth (an IdToken is obtained); CreateFunction of a python handler that prints "BACKEND-INVOKED <marker>" and returns 200; CreateRestApi; CreateAuthorizer(type="COGNITO_USER_POOLS", providerARNs=[<pool ARN>], identitySource="method.request.header.Authorization"); CreateResource(pathPart="secure"); PutMethod(httpMethod="GET", authorizationType="COGNITO_USER_POOLS", authorizerId=<id>); PutIntegration(type="AWS_PROXY", integrationHttpMethod="POST", uri=.../functions/<fn arn>/invocations); lambda:AddPermission for apigateway on arn:aws:execute-api:<region>:<account>:<api id>/*/*; CreateDeployment to stage "test".
  First observation: GET <invoke url>/secure with no Authorization header, polled until the status is one of {200, 401, 403}.
    Expected: 401, body containing "Unauthorized", and no "BACKEND-INVOKED <marker>" line in /aws/lambda/<fn> .
    Actual:   the poll never accepted a status; `Error: Check timed out after 60.0s`.
  Never reached, all behind that same first request: (b) Authorization "Bearer not-a-real-jwt.at.all" must also answer 401 (a 2xx would mean an arbitrary string was admitted as a pool token); the absence window asserting no BACKEND-INVOKED log line for either refused request; (c) the control — Authorization = the pool's real IdToken must answer 200 with body {"marker": "<marker>"} and the BACKEND-INVOKED log line must then appear (the control is what proves the asserted absence was the authorizer's decision and not a broken observation).
  The check's own poll budgets (150 s for the invoke URL, 120 s for the control's log line) are deadlines on propagation, not expected durations; the recording settles the first request well inside the 60 s the suite allows.

## Steps to Reproduce
1. CreateUserPool with a client; AdminCreateUser + AdminSetUserPassword(Permanent=true); AdminInitiateAuth to obtain an IdToken.
2. CreateFunction a python handler that prints "BACKEND-INVOKED <marker>" (marker from an env var) and returns {"statusCode": 200, "headers": {"Content-Type": "application/json"}, "body": "{\"marker\": \"<marker>\"}"}.
3. CreateRestApi; CreateAuthorizer(restApiId, type="COGNITO_USER_POOLS", providerARNs=[<pool ARN>], identitySource="method.request.header.Authorization"); CreateResource(pathPart="secure"); PutMethod(httpMethod="GET", authorizationType="COGNITO_USER_POOLS", authorizerId=<authorizer id>); PutIntegration(type="AWS_PROXY", integrationHttpMethod="POST", uri="arn:aws:apigateway:<region>:lambda:path/2015-03-31/functions/<fn arn>/invocations"); lambda AddPermission for Principal apigateway.amazonaws.com with SourceArn "arn:aws:execute-api:<region>:<account>:<api id>/*/*"; CreateDeployment to stage "test".
4. GET https://<api id>.execute-api.<region>.<host>/test/secure with NO Authorization header.
   Expected: HTTP 401 whose body carries API Gateway's Unauthorized envelope (contains "Unauthorized"); no "BACKEND-INVOKED <marker>" event in /aws/lambda/<fn>.
   Actual:   the request never settles on 200, 401 or 403; the scenario yields no verdict inside 60 s.
5. GET the same URL with header Authorization: "Bearer not-a-real-jwt.at.all".
   Expected: HTTP 401, and still no BACKEND-INVOKED log event.
6. GET the same URL with header Authorization: <the pool IdToken from step 1>.
   Expected: HTTP 200, body {"marker": "<marker>"}, and a "BACKEND-INVOKED <marker>" event in /aws/lambda/<fn>.
