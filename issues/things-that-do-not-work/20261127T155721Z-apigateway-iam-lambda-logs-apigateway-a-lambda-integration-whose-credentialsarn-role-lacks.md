Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__iam__lambda__logs.apigw-lambda-integration-credentials-role-without-invoke-rejected
Issue-Key: apigateway__iam__lambda__logs.apigw-lambda-integration-credentials-role-without-invoke-rejected|run#29|AssertionError|assert (status, text) == (500, '{"message": "Internal server error"}'), (
Issue-Commit: 7300602cd0fc37394a6328fdd869591f4a36c031

# apigateway: a Lambda integration whose credentialsArn role lacks lambda:InvokeFunction answers 200 with the IAM denial as the body, instead of 500 Internal server error

> _Note: this issue was identified against the code base at commit `7300602cd0fc37394a6328fdd869591f4a36c031`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1: a REST API AWS/AWS_PROXY integration configured with `credentials` (an IAM role ARN) that is not allowed `lambda:InvokeFunction` on the target function must surface as a backend failure, not as a successful response carrying the authorization text.

AWS: API Gateway assumes the integration's `credentials` role and calls Lambda with it. When that role is denied `lambda:InvokeFunction`, Lambda refuses the call and API Gateway reports the integration failure to the client as HTTP 500 with body `{"message": "Internal server error"}`; the IAM denial text appears only in the execution log, never in the response body. Recorded behavior only for the exact log wording; Smithy: the APIGateway model carries no response shape for a failed integration, so the requirement is the gateway's documented integration-failure mapping (500 / `Internal server error`).

Emulator: the request is answered HTTP 200 and the body is the raw IAM denial, `{"Message":"User: arn:aws:sts::<account>:assumed-role/<role-name>/BackplaneAssumeRoleSession is not authorized to perform: lambda:InvokeFunction on resource: arn:aws:lambda:<region>:<account>:function:<function-name> because no identity-based policy allo..."}` (the text is truncated mid-word in the body). Fixed text here is `is not authorized to perform: lambda:InvokeFunction on resource:` and `because no identity-based policy allows ...`; the role ARN, session name and function ARN are values from the request and from state. None of this belongs in a client-visible 200.

Why: a client cannot tell a successful invocation from a permission failure. Code that branches on HTTP status treats the denial as a successful integration and then parses the denial text as its payload; negative tests for integration credentials pass vacuously.

Fix: the REST API integration execution path — the component that assumes `credentials` and calls Lambda must map an authorization refusal from the downstream call to a 500 `Internal server error` gateway response and keep the denial text out of the body.

Traces: apigateway__iam__lambda__logs.apigw-lambda-integration-credentials-role-without-invoke-rejected.

## Evidence
Expected: HTTP 500, body {"message": "Internal server error"}.
Actual: HTTP 200, body {"Message":"User: arn:aws:sts::000000000000:assumed-role/sim2real-xsvc-cred-fd42042212ad-no/BackplaneAssumeRoleSession is not authorized to perform: lambda:InvokeFunction on resource: arn:aws:lambda:us-west-2:000000000000:function:sim2real-xsvc-cred-fd42042212ad because no identity-based policy allo'
Assertion: "a role without lambda:InvokeFunction should give 500 Internal server error, got 200".

## Steps to Reproduce
1. Create a Lambda function and a REST API with an AWS_PROXY integration whose Credentials is an IAM role that is NOT granted lambda:InvokeFunction on that function.
2. Deploy the stage and send a request to the invoke URL.
3. Observe HTTP 200 with the IAM denial text as the body instead of HTTP 500 {"message": "Internal server error"}.
