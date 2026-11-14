Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudformation__lambda.serverless-http-event-request-validator-rejects-missing-parameter
Issue-Key: apigateway__cloudformation__lambda.serverless-http-event-request-validator-rejects-missing-parameter|_npm_install_serverless#13|Timeout|r = subprocess.run(
Issue-Commit: 6cc05b66b5dc4f3acc804c4a889717eb31d245db

# apigateway__cloudformation__lambda: the Serverless-Framework http-event request-validator refusal scenario reaches no verdict — the deploy/serve path never completes inside the run budget

> _Note: this issue was identified against the code base at commit `6cc05b66b5dc4f3acc804c4a889717eb31d245db`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: the Serverless-Framework-deployed REST API whose http event declares a REQUIRED query string never reaches a verdict — the scenario is abandoned at the harness budget without the refusal or the success ever being graded.

AWS: a `serverless` http event with `request.parameters.querystrings.<name>: true` makes API Gateway attach a request validator to the method, so a call WITHOUT that query string is refused 400 with API Gateway's request-validation message and the function is NEVER invoked (no handler log line), while a call WITH it returns the handler's response body. The deploy itself goes through a CloudFormation stack (`<service>-<stage>`) that must reach CREATE_COMPLETE and publish the API/stage. Recorded behavior only for the exact 400 message text; the "required parameter missing is refused before the integration runs" rule is the documented one (https://docs.aws.amazon.com/apigateway/latest/developerguide/api-gateway-method-request-validation.html).

Emulator: the check ends with `Error: Check timed out after 60.0s` — neither the 400 refusal, nor the absence of an invocation log line, nor the served response is ever observed. The tooling precondition is satisfied (the check calls require_tools("npm", "node") first and did not fail on it), so the run got as far as the serverless deploy against the emulator and then never reached a gradeable state.

Why: a client deploying through the Serverless Framework — a mainstream path onto API Gateway — cannot confirm that its declared required query string is enforced at the method, nor that the function is shielded from the invalid call. The same deploy-never-settles family is already filed for this service's SAM/CDK siblings (20261105T022123Z-apigateway-cloudformation-lambda-4-net-new-root-causes.md, 20261108T120220Z-apigateway-cloudformation-lambda-non-proxy-aws-integration-with.md), neither of which carries this replay key.

Fix: the CloudFormation deploy path for a Serverless-Framework-generated stack reaching a terminal status and publishing a servable stage, plus the request-validator gate on the deployed method (it must refuse 400 before the integration is invoked). NOTE for the fixer: the only recorded symptom here is the exhausted budget; confirm the deploy settles before attributing the validator gate.

Traces: apigateway__cloudformation__lambda.serverless-http-event-request-validator-rejects-missing-parameter.

## Evidence
Replay (apigateway__cloudformation__lambda.serverless-http-event-request-validator-rejects-missing-parameter):
```
Error: Check timed out after 60.0s
```
Expected: the serverless service deploys (stack `<service>-dev` CREATE_COMPLETE, function `<service>-dev-echo`), a GET of /echo WITHOUT the `marker` query string is refused 400 with API Gateway's request-validation message and no INVOKED line appears in the function's log group, and a GET WITH `marker` returns {"echo": "<marker>"}. Actual: the run produces no graded response; it ends at the harness budget. (require_tools("npm", "node") passed, so the runner had its tooling.)

## Steps to Reproduce
1. In a work dir, npm install serverless@3.40.0, write handler.py (returns {"echo": queryStringParameters.marker} and prints "INVOKED <x-probe>") and a serverless.yml declaring: provider aws/python3.12/endpointType REGIONAL and a function `echo` with an `http` event (method get, path echo, authorizer aws_iam, request.parameters.querystrings.marker: true).
2. `serverless deploy` against the emulator; wait for the CloudFormation stack `<service>-dev` to reach CREATE_COMPLETE.
3. GET <stage>/echo with no query string: expect 400 with API Gateway's request-validation message and NO INVOKED line in the function's log group.
4. GET <stage>/echo?marker=<value>: expect 200 {"echo": "<value>"}. On the emulator none of these is reached before the budget is exhausted.
