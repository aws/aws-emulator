Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda.apigw-lambda-authorizer-missing-permission-negative apigateway__dynamodb__lambda.apigw-authorizer-deny-integration-not-invoked
Issue-Key: apigateway__dynamodb__lambda.apigw-lambda-authorizer-missing-permission-negative|run#19|AssertionError|assert (status, error_type) == (500, "AuthorizerConfigurationException"), (
Issue-Key: apigateway__dynamodb__lambda.apigw-authorizer-deny-integration-not-invoked|_collect_marker#27|Timeout|time.sleep(3.0)
Issue-Commit: 11bf1cc5011aa260df7304f2661cde6d9105badf

# apigateway__dynamodb__lambda: 2 net-new causes — an un-invocable Lambda authorizer answers InternalServerErrorException instead of AuthorizerConfigurationException, and the authorizer-Deny-blocks-the-integration check never completes

> _Note: this issue was identified against the code base at commit `11bf1cc5011aa260df7304f2661cde6d9105badf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAuthorizer

## Description
Two net-new root causes in the apigateway__dynamodb__lambda service. Both must be closed before this issue is submitted.

=== Cause 1 of 2: the missing-authorizer-invoke-permission refusal is the wrong error type and body ===
Failing check: apigateway__dynamodb__lambda.apigw-lambda-authorizer-missing-permission-negative

AWS: a REST method with authorizationType=CUSTOM and a TOKEN authorizer whose authorizerUri names a Lambda function that API Gateway has NO lambda:InvokeFunction grant for cannot be evaluated, and AWS answers its documented authorizer-configuration failure rather than a client-side refusal or the backend's response: HTTP 500 with header x-amzn-ErrorType: AuthorizerConfigurationException and body {"message": null}. Smithy (apigateway) models AuthorizerConfigurationException as a first-class error of the gateway's authorizer path; the status, the error type and the body are from the recorded response. Fixed text: the error-type token "AuthorizerConfigurationException" and the single-member body whose `message` is JSON null — there are no variable parts in this message. Granting lambda:InvokeFunction to apigateway.amazonaws.com for the authorizer's source ARN must make the identical call succeed, which is what distinguishes a correctly-enforcing gateway from one that is simply broken.

Emulator: answers HTTP 500 (status correct) but with x-amzn-ErrorType: InternalServerErrorException and body {"message":"Internal server error"} — the generic gateway 500, not the authorizer-configuration error. Both the error type and the message text diverge; the body also carries a string where AWS carries null.

Why: InternalServerErrorException reads as "the gateway or the backend broke, retry"; AuthorizerConfigurationException reads as "your authorizer is misconfigured, fix the permission". A client (or an SDK/retry layer keyed on x-amzn-ErrorType) retries a permanent configuration fault instead of surfacing it, and an operator debugging a 403/500 at the edge is pointed away from the missing lambda:InvokeFunction grant — the one thing that is wrong. A test asserting the documented authorizer-configuration failure cannot pass at all.

Fix: the execute-api authorizer evaluation path in the data plane — when the authorizer's Lambda cannot be invoked because no resource-policy grant permits apigateway.amazonaws.com, the response must be serialized as 500 with x-amzn-ErrorType: AuthorizerConfigurationException and body {"message": null}, not routed into the generic InternalServerErrorException gateway response.

=== Cause 2 of 2: the authorizer-Deny-does-not-invoke-the-integration contract never becomes observable ===
Failing check: apigateway__dynamodb__lambda.apigw-authorizer-deny-integration-not-invoked

AWS: a TOKEN authorizer that returns a policy with Effect=Deny for the method ARN makes the gateway answer 403 and NEVER invoke the integration, so the backend's side effect (a DynamoDB item written by the proxy-integration function) does not appear — the check clears the marker row first and then watches a window for its reappearance, so a reappearing row can only come from a new invocation. The companion Allow leg must answer 200 and leave the marker behind, which is what stops a uniformly-broken gateway from passing. Recorded behavior only; both the authorizer and the backend are granted lambda:InvokeFunction in this scenario, so a missing grant cannot be the reason the backend does not run.

Emulator: the check never completes — replay reports `Check timed out after 60.0s`, so neither the Allow control leg nor the Deny leg produced its outcome inside the cap. Hypothesis: given Cause 1 above, the authorizer invocation path does not complete for this wiring either, so the control GET never returns 200 with its marker and the check polls to the harness deadline instead of reaching the Deny assertion.

Why: this is the over-permissive direction — nothing establishes that a denied request fails to reach the backend. A client relying on a Lambda authorizer as its only authorization gate cannot tell, against this emulator, whether a Deny actually prevents the integration from running and writing to its table; a leak would be invisible.

Fix: the same execute-api authorizer evaluation path — the TOKEN authorizer must be invoked with the identitySource header, its returned policyDocument evaluated against the method ARN, a Deny answered as 403 before any integration call, and an Allow forwarded to the integration; the path must complete promptly rather than stalling.

## Evidence
Cause 1 — apigateway__dynamodb__lambda.apigw-lambda-authorizer-missing-permission-negative:

  File "run.py", line 445, in <lambda>
  File "check_apigateway__dynamodb__lambda_apigw_lambda_authorizer_missing_permission_negative.py", line 184, in run
  assert (status, error_type) == (500, "AuthorizerConfigurationException"), (
  AssertionError: expected 500 AuthorizerConfigurationException (missing authorizer invoke permission), got 500 'InternalServerErrorException': {"message":"Internal server error"}

The check reads the type from the response headers (`{k.lower(): v for k, v in headers.items()}.get("x-amzn-errortype")`) and then also asserts the body:
  assert json.loads(body) == {"message": None}

Expected: HTTP 500, x-amzn-ErrorType: AuthorizerConfigurationException, body {"message": null}.
Actual:   HTTP 500, x-amzn-ErrorType: InternalServerErrorException, body {"message":"Internal server error"}.
The status matches; the error type and the message text are both wrong, and the body's `message` is a string where AWS sends null. No part of this message is request- or state-derived — it is entirely fixed text.

The same check then calls lambda AddPermission to grant apigateway.amazonaws.com lambda:InvokeFunction on the authorizer and demands the identical GET now succeed, so a gateway that always answers 500 does not pass by accident.

Cause 2 — apigateway__dynamodb__lambda.apigw-authorizer-deny-integration-not-invoked:

  #### [mismatch] apigateway__dynamodb__lambda.apigw-authorizer-deny-integration-not-invoked
  Error: Check timed out after 60.0s

The check's awaited states: GET /guarded with Authorization "let-me-in" must answer 200 and the backend must write {pk: "invoked"} into the table (polled); the marker is then deleted; GET /guarded with Authorization "keep-me-out" must answer 403 and the marker must NOT reappear within the watched window. The authorizer is created with authorizerResultTtlInSeconds=0 so neither leg is served from the authorizer cache, and BOTH the authorizer and the backend are granted lambda:InvokeFunction.
Actual: no leg completed — the check was killed at 60.0s, so neither the 403 nor the no-invocation window was ever observed.

## Steps to Reproduce
Cause 1:
1. Create a REST API with a /secure resource and a TOKEN authorizer (identitySource=method.request.header.Authorization) whose authorizerUri points at a Lambda function; put a GET method with authorizationType=CUSTOM and authorizerId=<that authorizer>, wire an AWS_PROXY integration to a backend function, and CreateDeployment(stageName="test").
2. Do NOT grant the authorizer function a lambda:InvokeFunction permission for apigateway.amazonaws.com.
3. GET <stage>/secure with header Authorization: Bearer token123.
   Expected: HTTP 500, x-amzn-ErrorType: AuthorizerConfigurationException, body {"message": null}.
   Observed: HTTP 500, x-amzn-ErrorType: InternalServerErrorException, body {"message":"Internal server error"}.
4. lambda AddPermission(FunctionName=<authorizer>, Action=lambda:InvokeFunction, Principal=apigateway.amazonaws.com, SourceArn=arn:aws:execute-api:<region>:<account>:<api-id>/authorizers/<authorizer-id>) and repeat the identical GET — it must now succeed.

Cause 2:
1. Create a DynamoDB table (hash key pk) and wait for ACTIVE.
2. Create a backend Lambda (proxy integration) that PutItems {pk: "invoked"} and returns 200, and a TOKEN authorizer Lambda that returns Allow for token "let-me-in" and Deny otherwise for event.methodArn.
3. Create a REST API with /guarded GET, authorizationType=CUSTOM + the TOKEN authorizer (authorizerResultTtlInSeconds=0), AWS_PROXY integration to the backend, deploy to stage "test", and grant lambda:InvokeFunction to apigateway.amazonaws.com for BOTH functions.
4. GET <stage>/guarded with Authorization: let-me-in — expect 200 and the {pk: "invoked"} row; delete the row.
5. GET <stage>/guarded with Authorization: keep-me-out — expect 403, and the row must not reappear while watched.

Observed: the check never reaches step 5; it is killed by the 60 s harness cap.
