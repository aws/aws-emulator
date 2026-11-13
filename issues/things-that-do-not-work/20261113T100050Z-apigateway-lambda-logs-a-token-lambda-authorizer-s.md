Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__lambda__logs.apigw-lambda-authorizer-deny-blocks-backend-invocation
Issue-Key: apigateway__lambda__logs.apigw-lambda-authorizer-deny-blocks-backend-invocation|_stays_absent#22|Timeout|time.sleep(interval_s)
Issue-Commit: 522a89e9a46f5432e9084a06c26f1bc29a02dbde

# apigateway__lambda__logs: a TOKEN Lambda authorizer's Deny is still unobservable on a deployed REST stage — the invoke path reaches no 200/401/403 and the check hangs to the 60 s cap

> _Note: this issue was identified against the code base at commit `522a89e9a46f5432e9084a06c26f1bc29a02dbde`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
One net-new behavioral root cause, grounded in the single failing check of the simparity-behavioral-aws-v1 replay that is neither marked ALREADY FILED nor OWNED (apigateway__lambda__logs 2/5 measured here; the other failures in this service are OWNED by open issues and are not part of this one).

Named for the fixer, because neither covers this replay key: (a) 20261107T153156Z-apigateway-lambda-logs-a-rest-method-behind-a.md is marked FIXED and carried exactly this check id (Issue-Key ...|_stays_absent#22|Timeout|time.sleep(interval_s)) — its fix did NOT make the check reach a verdict: the same check still ends with 'Check timed out after 60.0s'; (b) 20261107T191316Z-apigateway-lambda-logs-the-lambda-authorizer-deny-check.md is open and names the same check id, but its replay key is a timeout inside _request_until_logged (the access-log leg) and the suite does not mark this failure as OWNED by it. This issue is filed for the failure as replayed now.

Cause 1 of 1 — a request to a deployed stage whose method is authorizationType=CUSTOM behind a TOKEN authorizer never produces an answer the authorizer's decision can be read from; the check exhausts its whole budget polling the stage invoke URL for any of 200/401/403.
Failing check id: apigateway__lambda__logs.apigw-lambda-authorizer-deny-blocks-backend-invocation

AWS: Smithy com.amazonaws.apigateway#CreateAuthorizer — required members restApiId/name/type (AuthorizerType enum, here TOKEN), with identitySource=method.request.header.Authorization and authorizerUri naming the authorizer function's arn:aws:apigateway:{region}:lambda:path/2015-03-31/functions/{functionArn}/invocations; the method's authorizationType is CUSTOM and its authorizerId names that authorizer; CreateDeployment publishes a stage. For a request to the stage's invoke URL, AWS invokes the authorizer, evaluates the returned policyDocument against the method ARN, and:
  - Effect Deny for the method ARN -> HTTP 403 with header x-amzn-errortype: AccessDeniedException and the explicit-deny body whose fixed text is "User is not authorized to access this resource with an explicit deny" (the body's only variable part is the request id the gateway mints; the check only requires the fixed substring "not authorized"), and the integration is NOT dispatched — observable as the absence of the backend function's own log line in /aws/lambda/{backendFunctionName};
  - Effect Allow -> the AWS_PROXY integration runs and its 200 body is returned;
  - an identity source the authorizer cannot evaluate -> 401.
So AWS always answers one of 200/401/403 for this wiring. The stage's accessLogSettings record carries $context.status "403", $context.path equal to the stage-qualified "/{stageName}/{resourcePath}" and a non-empty $context.requestId. Recorded behavior only for the timings (log delivery latency).

Emulator: nothing acceptable ever comes back. The replay ends with `Error: Check timed out after 60.0s` with no status, no error code and no body: the first probe of the stage invoke URL with the deny-triggering Authorization value polls for a status in {200, 401, 403} and never gets one inside the budget, so the deny assertions, the 'backend log group stayed empty' absence window, the 403 access-log assertions and the Allow-token control are all unreached. The emulator therefore never emits either the 403 AccessDeniedException refusal or an integration response for a CUSTOM-authorized method.

Why: a client cannot use a Lambda authorizer at all, and cannot distinguish 'the authorizer denied' from 'the gateway is broken' — a hang gives the caller no status to branch on, which is worse than a wrong status. Critically, the over-permissiveness this scenario exists to rule out (the backend integration running for a request the authorizer Denied) stays unverifiable, because neither leg of the differential answers.

Fix: the execute-api REST stage request path of the apigateway service — the authorizer invocation and policy-document evaluation for authorizationType=CUSTOM must reach a terminal decision (403 AccessDeniedException with the explicit-deny message on Deny, 401 on an unevaluable identity source, otherwise dispatch to the integration) instead of leaving the stage request unanswered; the stage's access-log and the backend function's execution-log delivery must settle so the 'not invoked' evidence is readable.

Traces: apigateway__lambda__logs.apigw-lambda-authorizer-deny-blocks-backend-invocation.

## Evidence
Behavioral replay (simparity-behavioral-aws-v1) against this checkout.

### apigateway__lambda__logs
[mismatch] apigateway__lambda__logs.apigw-lambda-authorizer-deny-blocks-backend-invocation
Error: Check timed out after 60.0s

The check creates a TOKEN authorizer (Allow only for the token "allow-me", Deny otherwise), a backend Lambda that prints "BACKEND-INVOKED <marker>", a /secure GET method with authorizationType=CUSTOM, the lambda:InvokeFunction permissions, a deployment to stage "test" and accessLogSettings with the format {"requestId":"$context.requestId","status":"$context.status","path":"$context.path"}. It then issues:
  deny_status, _, deny_text = _poll_http(url, accept=lambda s: s in (200, 401, 403), headers={"Authorization": "deny-me"}, ...)

Expected (AWS): 403, x-amzn-errortype: AccessDeniedException, body containing the fixed text "User is not authorized to access this resource with an explicit deny"; no BACKEND-INVOKED line in /aws/lambda/<backend> for that request; a 403 access-log record whose path is "/test/secure" and whose requestId is non-empty; and 200 carrying the backend marker for the "allow-me" token.
Actual: no status in {200, 401, 403} within the budget — the check reaches no assertion at all, so no status, no error code and no log evidence is produced (`Check timed out after 60.0s`). The previously FIXED issue 20261107T153156Z-apigateway-lambda-logs-a-rest-method-behind-a.md reported this same check timing out at 60.0s; the behaviour is unchanged.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Create a backend Lambda whose handler prints "BACKEND-INVOKED $MARKER" and returns {statusCode: 200, body: {"marker": $MARKER}}, and an authorizer Lambda returning a policyDocument with Effect Allow when authorizationToken == "allow-me" and Deny otherwise, Resource = event["methodArn"].
3. apigateway CreateRestApi; CreateResource(pathPart="secure"); CreateAuthorizer(type="TOKEN", identitySource="method.request.header.Authorization", authorizerUri=arn:aws:apigateway:<region>:lambda:path/2015-03-31/functions/<authorizerArn>/invocations, authorizerResultTtlInSeconds=0); PutMethod(httpMethod="GET", authorizationType="CUSTOM", authorizerId=<id>); PutIntegration(type="AWS_PROXY", integrationHttpMethod="POST", uri=...<backendArn>/invocations); lambda AddPermission for execute-api on both functions.
4. CreateDeployment(stageName="test") and UpdateStage with accessLogSettings naming a log group and a format containing $context.requestId, $context.status and $context.path.
5. GET https://<restApiId>.execute-api.<region>.amazonaws.com/test/secure with header `Authorization: deny-me`.
   Observed: the request never yields 200/401/403 within 60 s. AWS: 403 with the explicit-deny message and no backend invocation.
6. Repeat with `Authorization: allow-me`: AWS returns 200 with the backend marker.
7. Re-replay the check id in trace_ids to confirm a fix.
