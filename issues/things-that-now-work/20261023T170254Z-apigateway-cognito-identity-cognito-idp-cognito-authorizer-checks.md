Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cognito-identity__cognito-idp.apigw-authorizer-invoke-role-missing-invoke-permission-negative apigateway__cognito-identity__cognito-idp.apigw-authorizer-invoke-role-missing-trust-negative apigateway__cognito-identity__cognito-idp.apigw-authorizer-invoke-role-trust-apigateway-positive apigateway__cognito-identity__cognito-idp.apigw-cognito-authorizer-multiple-user-pools-audience-negative apigateway__cognito-identity__cognito-idp.apigw-cognito-user-pool-client-app-client-id-validation-positive
Issue-Key: apigateway__cognito-identity__cognito-idp|TestInvokeAuthorizer|NotImplemented
Issue-Key: apigateway__cognito-identity__cognito-idp|CreateAuthorizer|NotImplemented
Issue-Commit: f9ff4020ff731d5f2129f1ceb9f16685ad9310db

# apigateway__cognito-identity__cognito-idp: Cognito-authorizer checks abort because apigateway TestInvokeAuthorizer and CreateAuthorizer answer 501 NotImplemented (2 causes)

> _Note: this issue was identified against the code base at commit `f9ff4020ff731d5f2129f1ceb9f16685ad9310db`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TestInvokeAuthorizer

## Description
Two causes, both scaffold stubs on the API Gateway authorizer surface, blocking five Cognito-authorizer checks.

Cause 1 of 2: TestInvokeAuthorizer answers 501 NotImplemented, so an authorizer's invoke-role trust and invoke-permission behaviour cannot be exercised.
AWS: Smithy TestInvokeAuthorizer (POST /restapis/{restApiId}/authorizers/{authorizerId}, success 200) returns TestInvokeAuthorizerResponse (clientStatus, log, latency, principalId, policy, authorization, claims); modeled errors are BadRequestException (400), NotFoundException (404), TooManyRequestsException (429), UnauthorizedException (401). Real AWS evaluates the authorizer and reports the result there — including the failure the two negative checks expect when the authorizer's invoke role lacks lambda:InvokeFunction or lacks an apigateway.amazonaws.com trust relationship.
Emulator: HTTP 501, error code NotImplemented ('operation TestInvokeAuthorizer is scaffolded but not implemented in this emulator').
Why: a client gets an unmodeled 501 where AWS returns a decision payload or one of four modeled errors, so authorizer-role misconfiguration is undetectable through the API.
Fix: the TestInvokeAuthorizer handler must evaluate the stored authorizer and serialize TestInvokeAuthorizerResponse (services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/test_invoke_authorizer.rs).
Traces: apigateway__cognito-identity__cognito-idp.apigw-authorizer-invoke-role-missing-invoke-permission-negative, apigateway__cognito-identity__cognito-idp.apigw-authorizer-invoke-role-missing-trust-negative, apigateway__cognito-identity__cognito-idp.apigw-authorizer-invoke-role-trust-apigateway-positive.

Cause 2 of 2: CreateAuthorizer answers 501 NotImplemented on this path, so a COGNITO_USER_POOLS authorizer cannot be created at all.
AWS: CreateAuthorizer creates the authorizer and returns it with HTTP 201; its modeled failures are client 4xx (BadRequestException 400, ConflictException 409, NotFoundException 404, LimitExceededException/TooManyRequestsException 429, UnauthorizedException 401). Real AWS accepts a multi-user-pool providerARNs list and an app-client-id audience restriction, and refuses a bad providerARNs set with 400 BadRequestException.
Emulator: HTTP 501, error code NotImplemented ('operation CreateAuthorizer is scaffolded but not implemented in this emulator').
Why: no Cognito user-pool authorizer can be attached to a method, so neither the multi-pool audience negative nor the app-client-id validation positive is reachable; a client sees an unmodeled 501 instead of a 201 or a modeled 400.
Fix: the CreateAuthorizer handler and its providerARNs/audience validation (src/ops/create_authorizer.rs on the service that answered).
Traces: apigateway__cognito-identity__cognito-idp.apigw-cognito-authorizer-multiple-user-pools-audience-negative, apigateway__cognito-identity__cognito-idp.apigw-cognito-user-pool-client-app-client-id-validation-positive.

## Evidence
Cause 1 — Expected: HTTP 200 TestInvokeAuthorizerResponse (or a modeled 400/401/404/429). Actual: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the TestInvokeAuthorizer operation: operation TestInvokeAuthorizer is scaffolded but not implemented in this emulator (report lines 48402, 48617, 48833).
Cause 2 — Expected: HTTP 201 with the created authorizer, or 400 BadRequestException for an invalid providerARNs set. Actual: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateAuthorizer operation: operation CreateAuthorizer is scaffolded but not implemented in this emulator (report lines 49232, 49486).

## Steps to Reproduce
Cause 1: create a REST API with a TOKEN authorizer and an invoke role, then call apigateway test-invoke-authorizer --rest-api-id <id> --authorizer-id <id> --headers Authorization=<token>; expect HTTP 200 with a decision, observe HTTP 501 NotImplemented.
Cause 2: create a Cognito user pool, then call create-authorizer --type COGNITO_USER_POOLS --provider-arns <pool arn>; expect HTTP 201, observe HTTP 501 NotImplemented.
Re-replay the five check ids in trace_ids.
