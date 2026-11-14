Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudformation__lambda.serverless-http-event-request-validator-rejects-missing-parameter
Issue-Key: apigateway__cloudformation__lambda.serverless-http-event-request-validator-rejects-missing-parameter|_npm_install_serverless#13|Timeout|r = subprocess.run(
Issue-Commit: dbbfc43f271727daf6a36bf70721e9eb6030bb1c

# apigateway__cloudformation__lambda: the Serverless Framework HTTP-event request-validator scenario never reaches a verdict — the check is cut off at its 60 s budget

> _Note: this issue was identified against the code base at commit `dbbfc43f271727daf6a36bf70721e9eb6030bb1c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: a Serverless-Framework-deployed HTTP event with a request validator never settles, so the validator's refusal of a missing parameter is never observable.
AWS:      recorded behavior only. The check deploys a Serverless Framework service whose HTTP event declares request-parameter validation, then calls the deployed endpoint without the required parameter and expects API Gateway's request validator to refuse it (a 400 from the validator) while the conforming call succeeds. In AWS the deploy settles and both calls answer promptly, well inside the budget.
Emulator: the check ends with `Error: Check timed out after 60.0s` — no assertion is reached. The deploy and/or the execute-api call behind it never settles inside the budget, so the request-validator behaviour produces no verdict at all.
Why:      a client using the Serverless Framework (the most common non-CDK packaging of API Gateway + Lambda) cannot deploy a validated HTTP endpoint and see it enforce its parameter contract: the run hangs rather than returning either the validator's refusal or a successful call, so no request-validation behaviour can be developed or tested against the emulator.
Fix:      the CloudFormation provisioner's convergence for the Serverless-Framework-generated API Gateway + Lambda resource set (AWS::ApiGateway::RestApi/Resource/Method/RequestValidator/Deployment/Stage and AWS::Lambda::Function/Permission) and the execute-api data-plane path the check then calls — the deploy must reach a terminal status and the endpoint must answer instead of leaving the poller spinning.
Note on existing issues: this check is listed as unowned by the replay report. Open issues in this service family (for example 20261107T184737Z-apigateway-cloudformation-lambda-the-execute-api-data-plane-refuses-every-call.md) describe neighbouring execute-api failures but carry different replay keys and do not cover this check.
Traces:   apigateway__cloudformation__lambda.serverless-http-event-request-validator-rejects-missing-parameter.

## Evidence
#### [mismatch] apigateway__cloudformation__lambda.serverless-http-event-request-validator-rejects-missing-parameter
  Error: Check timed out after 60.0s

Expected: the Serverless deploy settles, the call missing the required request parameter is refused by the API Gateway request validator, and the conforming call succeeds.
Actual: the run is cut off at the 60 s budget before any assertion — the emulator produced no verdict for the request-validator behaviour.

## Steps to Reproduce
1. Deploy the suite's Serverless Framework service whose HTTP event declares request-parameter validation (`serverless deploy`, which issues CreateStack for the generated API Gateway + Lambda template).
2. Poll for the stack to reach a terminal status and the stage endpoint to answer.
3. Call the endpoint without the required request parameter, then with it.
4. Observe that the run never gets that far: the check is cut off at 60 s with no assertion reached.
