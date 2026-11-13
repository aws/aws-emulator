Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.cdk-deployed-l2-trio-serves-http-and-persists cloudformation.cdk-restapi-added-method-forces-a-new-deployment-that-serves
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.cdk-deployed-l2-trio-serves-http-and-persists|run#42|AssertionError|assert unmatched_answer == (502, b'{"message": "Internal server error"}'), (
Issue-Key: cloudformation.cdk-restapi-added-method-forces-a-new-deployment-that-serves|_cdk#44|AssertionError|assert result.returncode == expect_rc, (
Issue-Commit: 9821cff84aed55823f685374d459e562c1c72f3b

# cloudformation: 2 net-new causes — the API Gateway proxy 502 body drops AWS's space after the JSON colon, and an in-place update of AWS::ApiGateway::Stage is refused NotImplemented

> _Note: this issue was identified against the code base at commit `9821cff84aed55823f685374d459e562c1c72f3b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
Two independent root causes in the CloudFormation suite's CDK end-to-end checks. They are separate defects in separate components and must both be closed before this issue is submitted.

Cause 1 of 2: the API Gateway proxy-integration 502 gateway-response body is serialized without AWS's space after the JSON colon.
Failing check: cloudformation.cdk-deployed-l2-trio-serves-http-and-persists
  AWS:      when a Lambda proxy (AWS_PROXY) integration's handler raises, API Gateway answers the viewer HTTP 502 with its DEFAULT_5XX gateway-response body `{"message": "Internal server error"}`. The fixed text is the whole document, including the single space that follows the `:` separator; there is no variable part — the same bytes are returned for every failing proxy integration. Recorded behavior only (no Smithy model covers the execute-api data plane): the recording asserts exactly the tuple (502, b'{"message": "Internal server error"}').
  Emulator: the status 502 is correct, but the body is the compact form `{"message":"Internal server error"}` — no space after the colon, and therefore a Content-Length one byte shorter than AWS's.
  Why:      a client that byte-compares, hashes, or length-checks the gateway error body (contract tests, response-digest caches, proxies that re-frame the gateway error) sees a payload that is not the one AWS returns, even though the status code matches.
  Fix:      the execute-api data-plane front door's gateway-response serializer for API Gateway — the writer that emits the DEFAULT_5XX/5XX body. It must serialize the document with AWS's spacing rather than a compact JSON encoder.
  Traces:   cloudformation.cdk-deployed-l2-trio-serves-http-and-persists (the body-less request leg against the deployed LambdaRestApi).

Cause 2 of 2: CloudFormation refuses every in-place update of AWS::ApiGateway::Stage with an emulator-authored NotImplemented reason, so a CDK redeploy that adds a method cannot land.
Failing check: cloudformation.cdk-restapi-added-method-forces-a-new-deployment-that-serves
  AWS:      AWS::ApiGateway::Stage is an updatable resource type: changing its mutable properties (here the DeploymentId that a new AWS::ApiGateway::Deployment produces when a method is added) is applied in place and the stack reaches UPDATE_COMPLETE. Recorded behavior only for the emulator's refusal text; the requirement is that the update succeeds, so `cdk deploy` exits 0.
  Emulator: the stack's AWS::ApiGateway::Stage resource goes UPDATE_FAILED with the reason `NotImplemented: this emulator does not change the properties of an existing AWS::ApiGateway::Stage in place; the resource was left as it is.` The stack then reaches UPDATE_ROLLBACK_COMPLETE and `cdk deploy <stack> --require-approval never` exits 1 instead of 0. That reason string is emulator prose: AWS never emits a `NotImplemented:` resource-status reason for this type, and never reports that a resource "was left as it is".
  Why:      every template-managed change to an API Gateway stage is unreachable. Adding a method to a REST API — the ordinary CDK/CloudFormation redeploy — always rolls back, so a client can never evolve a deployed REST API through CloudFormation.
  Fix:      the CloudFormation AWS::ApiGateway::Stage resource handler's update path; it must apply mutable-property changes in place instead of returning a NotImplemented refusal.
  Related:  open issue 20261111T111216Z-apigateway-cloudformation-in-place-updates-of-aws-apigateway-deployment-method.md names Deployment, Method, Resource and Stage under the `apigateway__cloudformation` heading. It does not carry this check's replay key, so this cause is recorded here with the check that exercises it; closing that issue should be verified against this check too.
  Traces:   cloudformation.cdk-restapi-added-method-forces-a-new-deployment-that-serves.

## Evidence
Cause 1 — cloudformation.cdk-deployed-l2-trio-serves-http-and-persists [mismatch]:
  AssertionError: GET <distribution-of-stage>/_aws/execute-api/<apiId>/prod/<nopath> returned (502, b'{"message":"Internal server error"}'), expected the 502 API Gateway returns when the proxy-integrated handler raises on a body-less request
  Expected body: b'{"message": "Internal server error"}'  (space after the colon)
  Actual body:   b'{"message":"Internal server error"}'   (no space)
  Status 502 matches; only the serialized body diverges.

Cause 2 — cloudformation.cdk-restapi-added-method-forces-a-new-deployment-that-serves [mismatch]:
  AssertionError: `cdk deploy <stack> --require-approval never` exited 1, expected 0
  Stack event tail: UPDATE_FAILED | AWS::ApiGateway::Stage | Api/DeploymentStage.prod — "NotImplemented: this emulator does not change the properties of an existing AWS::ApiGateway::Stage in place; the resource was left as it is."
  Stack then reaches UPDATE_ROLLBACK_COMPLETE; expected UPDATE_COMPLETE.

## Steps to Reproduce
1. Clone the repo and run the behavioral suite (run_behavioral_suite).
2. Cause 1: deploy the CDK app with a Lambda Function + DynamoDB Table + LambdaRestApi (check `cdk-deployed-l2-trio-serves-http-and-persists`), then GET a path on the deployed `prod` stage that makes the proxy handler raise (a body-less request to an unmatched path). Compare the response body bytes to b'{"message": "Internal server error"}'.
3. Cause 2: deploy the CDK REST API app from `cdk-restapi-added-method-forces-a-new-deployment-that-serves` with one GET method, then add a second GET method and `cdk deploy <stack> --require-approval never` again. Observe the AWS::ApiGateway::Stage UPDATE_FAILED NotImplemented reason and the UPDATE_ROLLBACK_COMPLETE instead of UPDATE_COMPLETE.
