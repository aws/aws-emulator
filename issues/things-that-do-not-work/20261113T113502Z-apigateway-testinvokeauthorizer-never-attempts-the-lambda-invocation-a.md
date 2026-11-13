Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.test-invoke-authorizer-enforces-identity-source apigateway.test-invoke-authorizer-missing-identity-source-fails apigateway.test-invoke-authorizer-missing-identity-source-rejected
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway.test-invoke-authorizer-enforces-identity-source|run#18|AssertionError|assert "sim2real-nonexistent-authorizer" in log, (
Issue-Key: apigateway.test-invoke-authorizer-missing-identity-source-fails|run#20|AssertionError|assert "sim2real-fidelity-nonexistent" in present_log, (
Issue-Key: apigateway.test-invoke-authorizer-missing-identity-source-rejected|run#20|AssertionError|assert function_name in present_log, (
Issue-Commit: 2bc4f9350f4794788afdc3a762d704c686e40d96

# apigateway TestInvokeAuthorizer never attempts the Lambda invocation: a pre-invoke permission check short-circuits with an emulator-authored configuration-error sentence that never names the backing function

> _Note: this issue was identified against the code base at commit `2bc4f9350f4794788afdc3a762d704c686e40d96`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TestInvokeAuthorizer

## Description
One net-new root cause in apigateway TestInvokeAuthorizer, grounded in 3 failing checks of the simparity-behavioral-aws-v1 replay (apigateway 455/580). Every other apigateway failure in this report is marked OWNED by an open issue or ALREADY FILED and is NOT part of this issue. Related open issues that do NOT carry these three replay keys, named for the fixer: 20261111T130919Z-apigateway-testinvokeauthorizer-a-deny-decision-is-reported-as.md (clientStatus for an EXECUTED Deny, the ESTestInvoke-stage spelling, and the Cognito-only `claims` member — different members, and all of them on the path where the function DID run), 20261111T130919Z-apigateway-cognito-identity-cognito-idp-testinvokeauthorizer-ignores-authorizer.md (authorizerCredentials is ignored) and 20261029T200808Z-apigateway-api-gateway-authorizer-representation-and-testinvokemethod-execution-9.md (authorizer representation and TestInvokeMethod). None of them covers the pre-invoke short-circuit described here.

Cause 1 of 1: for a TOKEN authorizer whose identitySource IS present on the simulated request, TestInvokeAuthorizer answers the configuration-error path WITHOUT ever attempting to invoke the authorizer's Lambda function, and the execution log it returns never references that function.

AWS: Smithy com.amazonaws.apigateway#TestInvokeAuthorizer -> TestInvokeAuthorizerResponse members clientStatus (Integer, default 0), log (String), latency (Long), principalId, policy, authorization, claims; the operation is POST /restapis/{restApiId}/authorizers/{authorizerId} and models BadRequestException(400)/NotFoundException(404)/TooManyRequestsException(429)/UnauthorizedException(401). The model says nothing about the CONTENT of `log`, so for the log this is recorded behavior only: once the identity source the authorizer declares is carried by the simulated request, API Gateway proceeds to the invocation of the function named by Authorizer$authorizerUri, and the execution log records that attempt — it carries the authorizerUri/function ARN (the 'Sending request to https://lambda.<region>.amazonaws.com/2015-03-31/functions/<function-arn>/invocations' line), so the function name appears in the log. Whether the function may be invoked is answered BY Lambda, as the response to that invoke; a function that does not exist answers it 404 ResourceNotFoundException. The configuration-error sentence AWS words for an authorizer whose function cannot be invoked is the fixed text 'Execution failed due to configuration error: Invalid permissions on Lambda function' (no variable parts) — it never names the API's authorizer.

Emulator: the handler asks a resource-policy precheck BEFORE invoking (services/service-com-amazonaws-apigateway-backplanecontrolservice/src/service/invoke/custom_authorizer.rs lines 386-401: `invocation_admitted(client, function, &pending.authorizer_arn)`; its negative answer is turned into a reason string, rendered at src/service/invoke/simulated_authorizer.rs lines 259-264 as 'Execution failed due to configuration error: {reason}'). Because the precheck fails, no invoke is ever issued, so the log contains no reference to the function at all; the log ends with the emulator-authored sentence 'Execution failed due to configuration error: the authorizer's function allows this api's authorizer <authorizerName> no invocation of it, so no invocation was made' followed by 'Answered through this API's AUTHORIZER_CONFIGURATION_ERROR gateway response: the client would have received HTTP 500'. The fixed text of that sentence is emulator-authored (it is not AWS's wording for this class) and its one variable part is the API's authorizer NAME, which AWS's message does not carry.

Why: TestInvokeAuthorizer exists so a developer can see WHETHER and HOW the authorizer function ran. A client reading the returned log cannot tell an authorizer that was invoked and failed from one that was never invoked, and cannot see which function was addressed — the single most useful datum in the log. Worse, the identity-source gate becomes unobservable: the log is identical whether the identity source was absent (correctly short-circuited) or present (should have been invoked), so a developer validating that the identity source gates invocation gets the same evidence either way. And because a simulation refuses on an API-Gateway-side permission guess instead of on Lambda's own answer, an authorizer the real service would have invoked is reported as a configuration error, and the diagnosis text a client greps for ('Invalid permissions on Lambda function') never appears.

Fix: the simulated-authorizer invocation path in the apigateway crate — the CUSTOM-authorizer leg must issue the invocation against Authorizer$authorizerUri and record that attempt (naming the function/URI) in the execution log before any verdict, and derive the configuration-error outcome from the invoke's own failure rather than from a pre-invoke resource-policy precheck; the refusal sentence must be AWS's fixed text for the class. File/line citations: services/service-com-amazonaws-apigateway-backplanecontrolservice/src/service/invoke/custom_authorizer.rs lines 386-401 (the precheck and the authored reason), src/service/invoke/simulated_authorizer.rs lines 184-265 (the step log and the Unobtained branch).

Traces: apigateway.test-invoke-authorizer-enforces-identity-source, apigateway.test-invoke-authorizer-missing-identity-source-fails, apigateway.test-invoke-authorizer-missing-identity-source-rejected. In all three the NEGATIVE leg (identity source absent -> clientStatus 401, no principalId, no policy, function not named in the log) already passes; only the positive control, with the identity source present, diverges.

Not filed from these checks: the per-run timestamps inside the log text and the generated api/authorizer ids and authorizer names — those are per-run values, not requirements.

## Evidence
Behavioral replay (simparity-behavioral-aws-v1) against this checkout; apigateway 455/580.

[mismatch] apigateway.test-invoke-authorizer-enforces-identity-source
  AssertionError: expected the log to reference the (nonexistent) authorizer function once identitySource is present, proving an invocation was actually attempted, got:
  "Execution log for request test-invoke-request\n<ts> : Starting execution for request: test-invoke-request\n<ts> : Authorizer: <authorizerId>, type: TOKEN, identity source: method.request.header.Authorization\n<ts> : Method request path: /\n<ts> : Method request headers: [Authorization]\n<ts> : Method request body before transformations: 0 byte(s)\n<ts> : Starting authorizer: <authorizerId> of type TOKEN for the simulated request\n<ts> : Execution failed due to configuration error: the authorizer's function allows this api's authorizer <authorizerName> no invocation of it, so no invocation was made\n<ts> : Answered through this API's AUTHORIZER_CONFIGURATION_ERROR gateway response: the client would have received HTTP 500"

[mismatch] apigateway.test-invoke-authorizer-missing-identity-source-fails
  AssertionError: expected the log to reference the backing function once the identity source header is present, proving an invocation was actually attempted — same log shape as above.

[mismatch] apigateway.test-invoke-authorizer-missing-identity-source-rejected
  AssertionError: expected an actual invocation attempt against 'sim2real-nonexistent' once the identity source header is present, but the log does not reference it — same log shape as above.

Expected (AWS): with the identitySource header present, the execution log records the invocation attempt against the function named by authorizerUri (the function ARN / the lambda invocations URI appears in the log), and a function that cannot be invoked ends the log with the fixed text 'Execution failed due to configuration error: Invalid permissions on Lambda function'.
Actual (emulator): no invoke is attempted, the function never appears in the log, and the log ends with the emulator-authored sentence quoted above whose only variable part is the API's authorizer name.

Per-run values in the quoted log (timestamps shown as <ts>, generated authorizer id and authorizer name) are NOT part of the requirement; the presence of the function reference and the wording of the configuration-error sentence are.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. CreateRestApi.
3. CreateAuthorizer on that API with type=TOKEN, identitySource=method.request.header.Authorization, authorizerResultTtlInSeconds=0 and authorizerUri=arn:aws:apigateway:<region>:lambda:path/2015-03-31/functions/arn:aws:lambda:<region>:<caller-account-id>:function:does-not-exist/invocations (the function deliberately does not exist; CreateAuthorizer only validates ARN shape).
4. apigateway test-invoke-authorizer --rest-api-id <apiId> --authorizer-id <authorizerId> --headers {} -> clientStatus 401 and a log that does not name the function. This leg is correct.
5. apigateway test-invoke-authorizer --rest-api-id <apiId> --authorizer-id <authorizerId> --headers Authorization=some-token -> observe the returned `log`: it contains no reference to 'does-not-exist' and ends with 'Execution failed due to configuration error: the authorizer's function allows this api's authorizer <authorizerName> no invocation of it, so no invocation was made'. AWS records the invocation attempt against the function (so the function appears in the log) and words the failure 'Execution failed due to configuration error: Invalid permissions on Lambda function'.
6. Re-replay the three check ids listed in trace_ids.
