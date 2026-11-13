Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cognito-identity__cognito-idp.deploy-apigw-execution-role-trust-policy-template-positive
Issue-Key: apigateway__cognito-identity__cognito-idp.deploy-apigw-execution-role-trust-policy-template-positive|_poll_until#7|Timeout|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Commit: 4ee504be713ad5d2fe853da15ad6096ca014344c

# apigateway__cognito-identity__cognito-idp: the CFN-deployed TOKEN-authorizer stage check now consumes the whole 60 s harness budget without ever reaching an Allow/Deny decision (1 cause)

> _Note: this issue was identified against the code base at commit `4ee504be713ad5d2fe853da15ad6096ca014344c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAuthorizer

## Description
One behavioral root cause, grounded in the single non-owned failing check of the simparity-behavioral-aws-v1 replay under the apigateway__cognito-identity__cognito-idp heading (12/22 passed; the other nine failures in this service are marked OWNED by open issues and are NOT part of this issue). Related OPEN issue that names this same check but does NOT carry its current replay key: 20261108T125359Z-apigateway-cognito-identity-cognito-idp-a-cfn-deployed.md (there the stage DID answer, with a persistent 500 {"message":"Internal server error"}); the failure signature has since changed from that AssertionError to a harness-level budget cut, so the check is no longer covered by it and must be re-replayed on its own. Also related, different service tokens / different symptoms: 20261107T153156Z-apigateway-lambda-logs-a-rest-method-behind-a.md and 20261027T030000Z-apigateway-a-token-lambda-authorizer-allow-decision-is-never-obtained.md.

Cause 1 of 1: a CloudFormation-provisioned REST API whose method is gated by a TOKEN Lambda authorizer with authorizerCredentials never produces a decidable answer on its deployed stage — the whole check is cut at the 60 s cap with no leg completed.
AWS: Smithy com.amazonaws.apigateway#CreateAuthorizer, member `authorizerCredentials` — "Specifies the required credentials as an IAM role for API Gateway to invoke the authorizer. To specify an IAM role for API Gateway to assume, use the role's Amazon Resource Name (ARN). To use resource-based permissions on the Lambda function, specify null." With type TOKEN, identitySource method.request.header.Authorization, authorizerResultTtlInSeconds 0, a method whose authorizationType is CUSTOM naming that authorizer, a MOCK integration returning {"message":"ok"}, and a deployed stage, AWS invokes the authorizer under that credentials role (no Lambda resource policy needed, because the role itself grants lambda:InvokeFunction on the function) and honours the returned policyDocument: GET /<stage>/secure with Authorization: allow-me -> 200 with the integration body {"message":"ok"} (fixed text from the template), and Authorization: deny-me -> 403 for the explicit Deny. AWS answers 500 with the fixed generic body {"message":"Internal server error"} ONLY when the authorizer cannot be invoked at all (untrusted or unauthorised authorizerCredentials) — the case the companion negative checks cover, and the case that must NOT fire for this correctly-permissioned wiring.
Emulator: the check never reaches either assertion. It is terminated by the harness with `Error: Check timed out after 60.0s`, so neither the Allow leg (expected 200/{"message":"ok"}) nor the Deny leg (expected 403) is ever observed. The same check previously recorded a persistent 500 {"message":"Internal server error"} on every 3 s probe for the full poll window, i.e. the Allow/Deny decision was never obtained then either; it is now not obtained inside the budget at all. Hypothesis: either CloudFormation provisioning of this eight-resource stack (two IAM roles, a python3.12 Lambda, RestApi + Resource + Authorizer + Method, Deployment, Stage) or the execute-api stage request path blocks long enough to exhaust the budget before the first probe can settle; the replay output carries only the budget cut, so the exact blocking step is not stated by the recording.
Why: a client cannot use the documented authorizerCredentials alternative to a Lambda resource policy — no Allow decision ever reaches an integration and no explicit Deny ever produces its 403. Worse, a correctly-permissioned authorizer is indistinguishable from a misconfigured one: the negative checks in this suite assert the configuration-error 500 for a role WITHOUT the invoke grant, so a caller or security test comparing the two configurations cannot tell a working authorizer from a broken one, and now cannot get any answer in time at all.
Fix: the apigateway crate's execute-api REST stage request path (authorizer invocation and policy evaluation) together with the CloudFormation provisioning of AWS::ApiGateway::{Authorizer,Method,Deployment,Stage} — the stage must accept an authorizerCredentials role that trusts apigateway.amazonaws.com and carries lambda:InvokeFunction on the authorizer function, invoke the authorizer, evaluate the returned policyDocument into either the integration call (200) or a 403, and do so promptly instead of classifying the role as unusable or leaving the request unanswered.
Traces: apigateway__cognito-identity__cognito-idp.deploy-apigw-execution-role-trust-policy-template-positive.

## Evidence
Behavioral replay (simparity-behavioral-aws-v1) against this checkout.

[mismatch] apigateway__cognito-identity__cognito-idp.deploy-apigw-execution-role-trust-policy-template-positive
  Error: Check timed out after 60.0s

What the check does: create_stack (CAPABILITY_IAM, CAPABILITY_NAMED_IAM) with LambdaExecutionRole (trusts lambda.amazonaws.com, AWSLambdaBasicExecutionRole), AuthorizerFunction (python3.12, returns a policyDocument with Effect Allow only when event['authorizationToken'] == 'allow-me', Deny otherwise), ApiGatewayInvokeRole (AssumeRolePolicyDocument trusting apigateway.amazonaws.com, inline policy allowing lambda:InvokeFunction on !GetAtt AuthorizerFunction.Arn), RestApi, a /secure resource, TokenAuthorizer (Type TOKEN, AuthorizerUri arn:<partition>:apigateway:<region>:lambda:path/2015-03-31/functions/<fnArn>/invocations, AuthorizerCredentials !GetAtt ApiGatewayInvokeRole.Arn, IdentitySource method.request.header.Authorization, AuthorizerResultTtlInSeconds 0), SecureMethod (GET, AuthorizationType CUSTOM, MOCK integration with IntegrationResponses 200 -> {"message":"ok"}), ApiDeployment and ApiStage 'test'; then it polls GET <stage>/secure with Authorization: allow-me every 3 s until the status is no longer 500, asserts 200 and body {"message":"ok"}, and finally asserts 403 for Authorization: deny-me.

Expected (AWS):
  GET /test/secure, Authorization: allow-me  -> 200, body {"message":"ok"}  (authorizer Allow, MOCK integration reached)
  GET /test/secure, Authorization: deny-me   -> 403                        (authorizer explicit Deny)
Actual (emulator):
  no answer is graded at all — the check is cut by the harness after 60.0 s, so the Allow leg never settles and the Deny leg is never sent.
  (Prior replay of this same check, filed as 20261108T125359Z-apigateway-cognito-identity-cognito-idp-a-cfn-deployed.md: 500 with body {"message":"Internal server error"} on every probe for the whole poll window — the decision was never obtained then either.)

Only the statuses and the fixed body texts {"message":"ok"} / {"message":"Internal server error"} are requirements; the api id, role names, function name, stage host and timings are per-run values.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. cloudformation create-stack --capabilities CAPABILITY_IAM CAPABILITY_NAMED_IAM with a template containing: an IAM role trusting lambda.amazonaws.com for the authorizer function; a python3.12 Lambda whose handler returns a policyDocument with Effect Allow when event['authorizationToken'] == 'allow-me' and Deny otherwise; an IAM role whose AssumeRolePolicyDocument trusts apigateway.amazonaws.com with an inline policy allowing lambda:InvokeFunction on that function's ARN; an AWS::ApiGateway::RestApi with a /secure resource; an AWS::ApiGateway::Authorizer of Type TOKEN with AuthorizerUri arn:aws:apigateway:<region>:lambda:path/2015-03-31/functions/<fnArn>/invocations, AuthorizerCredentials set to the apigateway-trusting role ARN, IdentitySource method.request.header.Authorization, AuthorizerResultTtlInSeconds 0; an AWS::ApiGateway::Method GET with AuthorizationType CUSTOM, that AuthorizerId and a MOCK integration returning {"message":"ok"}; a Deployment and a Stage named 'test'; Outputs.ApiId = !Ref RestApi.
3. Wait for the stack to reach CREATE_COMPLETE and read the ApiId output. Note how long provisioning takes — the whole scenario must fit a 60 s client budget.
4. curl -H 'Authorization: allow-me' <stage invoke url>/test/secure, repeating every 3 s.
   Observed: no 200 is ever produced within 60 s (the check is cut at the cap; previously every probe answered 500 {"message":"Internal server error"}). AWS: 200 {"message":"ok"}.
5. curl -H 'Authorization: deny-me' <same url>.
   AWS: 403. (Unreachable today — step 4 never clears.)
6. Re-replay the check id in trace_ids.
