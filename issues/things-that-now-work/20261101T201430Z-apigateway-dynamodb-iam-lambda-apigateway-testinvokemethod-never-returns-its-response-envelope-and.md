Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__iam__lambda.test-invoke-is-rejected-by-a-lambda-permission-scoped-to-the-beta-stage
Issue-Key: apigateway__dynamodb__iam__lambda.test-invoke-is-rejected-by-a-lambda-permission-scoped-to-the-beta-stage|_await#5|Timeout|last = probe()
Issue-Commit: dc4158ff05b54d9987477de08887f7f0381de207

# apigateway: TestInvokeMethod never returns its response envelope and the AWS_IAM-authorized deployed stage never answers, so the check hangs to the harness cap

> _Note: this issue was identified against the code base at commit `dc4158ff05b54d9987477de08887f7f0381de207`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TestInvokeMethod

## Description
Cause 1 of 1: neither a SigV4-signed request to a deployed REST API stage whose method is AWS_IAM-authorized, nor TestInvokeMethod against that method's AWS_PROXY (Lambda) integration, ever produces a response; the check exhausts the harness budget without observing any status, error code or body. Failing check: apigateway__dynamodb__iam__lambda.test-invoke-is-rejected-by-a-lambda-permission-scoped-to-the-beta-stage.

AWS: Smithy apigateway TestInvokeMethod — POST /restapis/{restApiId}/resources/{resourceId}/methods/{httpMethod}, with restApiId/resourceId/httpMethod as @httpLabel @required plus pathWithQueryString, body, headers, multiValueHeaders, clientCertificateId and stageVariables; output status (Integer), body (String), headers, multiValueHeaders, log (String), latency (Long); modeled errors BadRequestException 400, UnauthorizedException 401, NotFoundException 404, TooManyRequestsException 429. It is a synchronous control-plane call that ALWAYS answers: HTTP 200 carrying that envelope, or one of those four modeled errors. It never leaves the caller without a response. Recorded behavior only for the semantics this check grades: TestInvokeMethod bypasses the method's AWS_IAM authorization but NOT the integration target's own resource policy, and it invokes the function as if from the reserved stage segment "test-invoke-stage". With the function's Lambda resource policy granting apigateway.amazonaws.com only SourceArn arn:aws:execute-api:<region>:<account>:<restApiId>/<deployed-stage>/GET/<path-prefix>/* , the test invoke answers with envelope status 500 and body exactly {"message": "Internal server error"} — the fixed text is "Internal server error" under the JSON member "message", with no variable part. After AddPermission adds a statement whose SourceArn carries the reserved "test-invoke-stage" segment in place of the deployed stage name, the same test invoke answers status 200 with the integration's JSON body. Independently, a SigV4-signed GET to the deployed stage invoke URL from a role allowed execute-api:Invoke on <api-arn>/<deployed-stage>/GET/* is served 200 throughout.

Emulator: nothing comes back. The replay detail is exactly "Error: Check timed out after 60.0s". The check's first blocking wait is the signed AWS_IAM stage GET (_await(..., "the signed stage GET to be served"), its own deadline 240 s), and its later loop polls TestInvokeMethod for status 200; neither converges, so the harness cap fires before any HTTP status, error code or body exists to compare.

Why: a client gets no answer where AWS always answers, and an SDK call blocks until its own socket timeout instead of being served or refused. Two contracts become unobservable: that TestInvokeMethod is synchronous and returns the status/body/headers/log/latency envelope (or a modeled 400/401/404/429), and that a test invoke is still gated by the integration target's resource policy and attributed to the reserved "test-invoke-stage" segment — so a caller cannot distinguish the permission-scoped refusal (status 500, body {"message": "Internal server error"}) from a success.

Fix: the apigateway TestInvokeMethod handler and the integration-invocation path it shares with the execute-api data plane — TestInvokeMethod must always synthesize and return its response envelope, it must evaluate the Lambda function's resource policy against the synthetic source ARN built with the reserved "test-invoke-stage" segment, and the execute-api data plane must always produce an HTTP response for a signed AWS_IAM request to a deployed stage instead of never replying.

Related open issues, named because they do not own this check in the replay report: 20261030T191000Z-apigateway-dynamodb-iam-lambda-the-testinvokemethod-vs-lambda.md lists this same check in its Issue-Trace-Ids but its Issue-Key pins a different step (_session#5, the AssumeRole inside _session), so this run's failure was not matched to it and the report did not mark the check as owned; 20261023T210711Z-apigateway-dynamodb-iam-lambda-the-iam-authorization-lifecycle.md carries only the sibling check e2e-debug-api-gateway-iam-authorization-lifecycle and says nothing about TestInvokeMethod, the reserved test-invoke-stage source ARN, or the 500 {"message": "Internal server error"} refusal asserted here. If a fixer closes this, those two should be reconciled against the same handler rather than fixed separately.

Traces: apigateway__dynamodb__iam__lambda.test-invoke-is-rejected-by-a-lambda-permission-scoped-to-the-beta-stage.

## Evidence
Replay verdict: [mismatch] apigateway__dynamodb__iam__lambda.test-invoke-is-rejected-by-a-lambda-permission-scoped-to-the-beta-stage
Detail, verbatim and complete: `Error: Check timed out after 60.0s`

Expected (AWS, per the check's assertions and the Smithy model):
- A SigV4-signed GET to <stage-invoke-url>/<path>/<id> from a role allowed execute-api:Invoke on <api-arn>/<deployed-stage>/GET/* returns HTTP 200 (asserted for 2 consecutive probes).
- GetPolicy on the function lists exactly one statement whose Condition.ArnLike."AWS:SourceArn" is <api-arn>/<deployed-stage>/GET/<path-prefix>/*.
- TestInvokeMethod(restApiId, resourceId, httpMethod=GET, pathWithQueryString=/<path>/<id>) returns an envelope with status == 500 and body == {"message": "Internal server error"} (fixed text "Internal server error" under member "message"; no variable part), because the only grant names the deployed stage and not the reserved test-invoke-stage.
- After AddPermission with SourceArn <api-arn>/test-invoke-stage/GET/<path-prefix>/*, TestInvokeMethod returns status == 200 and body equal to the stored item's JSON.

Actual (emulator): no response at any of those steps. No HTTP status, no error code and no body were produced within the 60 s harness cap, so none of the assertions above was reached. TestInvokeMethod is modeled to answer 200 with the status/body/headers/log/latency envelope or one of BadRequestException(400) / UnauthorizedException(401) / NotFoundException(404) / TooManyRequestsException(429); it answered none of these.

## Steps to Reproduce
1. CreateTable with a single HASH key attribute, PAY_PER_REQUEST, and PutItem one item.
2. CreateRole trusting lambda.amazonaws.com with read access to that table; CreateFunction (python3.12) returning a Lambda-proxy response for the item keyed by the path parameter.
3. CreateRestApi (REGIONAL), create nested resources /<collection>/{id}, PutMethod GET with authorizationType=AWS_IAM, PutIntegration type=AWS_PROXY integrationHttpMethod=POST targeting the function.
4. Lambda AddPermission for Principal apigateway.amazonaws.com with SourceArn arn:aws:execute-api:<region>:<account>:<restApiId>/<deployed-stage>/GET/<collection>/* ONLY (no test-invoke-stage grant); CreateDeployment with that stageName.
5. CreateRole trusted by the account and allowed execute-api:Invoke on arn:aws:execute-api:<region>:<account>:<restApiId>/<deployed-stage>/GET/*; AssumeRole and send a SigV4-signed GET to the stage invoke URL. Observe that no HTTP response arrives.
6. Call TestInvokeMethod(restApiId, resourceId of {id}, httpMethod=GET, pathWithQueryString=/<collection>/<id>). Observe that it does not return the status/body envelope (AWS: status 500, body {"message": "Internal server error"}).
7. AddPermission a second statement with SourceArn .../test-invoke-stage/GET/<collection>/* and repeat step 6 (AWS: status 200 with the item JSON).
8. Or replay apigateway__dynamodb__iam__lambda.test-invoke-is-rejected-by-a-lambda-permission-scoped-to-the-beta-stage.
