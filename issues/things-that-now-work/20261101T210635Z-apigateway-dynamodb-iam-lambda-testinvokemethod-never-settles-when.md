Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__iam__lambda.test-invoke-is-rejected-by-a-lambda-permission-scoped-to-the-beta-stage
Issue-Key: apigateway__dynamodb__iam__lambda.test-invoke-is-rejected-by-a-lambda-permission-scoped-to-the-beta-stage|_session#5|Timeout|return Credentials(c["AccessKeyId"], c["SecretAccessKey"], c["SessionToken"])
Issue-Commit: 62e4c5cd3531cb5ea16934ee4e8b2e43c8dd65a3

# apigateway__dynamodb__iam__lambda: TestInvokeMethod never settles when the Lambda resource policy is scoped to one stage, so the stage-scoped permission check hangs to the harness budget

> _Note: this issue was identified against the code base at commit `62e4c5cd3531cb5ea16934ee4e8b2e43c8dd65a3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Resolution
Fixed. The code base drifted after this report was filed: `TestInvokeMethod` reaches a terminal answer for a Lambda-proxy method out of the function's own resource policy, and the recorded unit this report names replays as passed. `services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/test_invoke_method.rs` performs the method's integration through `crate::service::invoke::perform_integration` — the same function the `execute-api` data plane serves a real request through — naming the simulated call by the method ARN under the stage AWS reserves for a test invocation (`test-invoke-stage`), and `src/service/invoke/lambda_proxy.rs` decides the invocation by reading the function's policy from Lambda itself (`GetPolicy`) and judging the grant's `aws:SourceArn` against that ARN: a function admitting API Gateway nothing for it is not invoked, and the simulation is answered through the api's `API_CONFIGURATION_ERROR` gateway response (500 with `{"message":"Internal server error"}`) with the configuration error named in the execution log.

What this change adds is the coverage no existing suite carried: `tests/all/test_invoke_stage_grant_differential.rs`, on the harness `tests/all/test_invoke_method_lambda_permission.rs` publishes, pins the CROSSING of the method's own gate with the function's policy — over `Method$authorizationType` `NONE` and `AWS_IAM` and `Method$apiKeyRequired`, each simulated through a function whose only grant names the deployed stage (refused 500, `Invalid permissions on Lambda function` in the execution log, no run logged) and then through the same function once a grant for the reserved `test-invoke-stage` ARN is added beside it (the handler's own 200, one run logged) — and the SEQUENCE a client follows when a test invoke is refused: the added grant admits the next simulation while the deployed caller the policy already admitted answers exactly as it did before. Which stage SEGMENT each grant names is `tests/all/test_invoke_method_stage_segment_grants.rs`'s table.

## Operation
TestInvokeMethod

## Description
Cause 1 of 1: a REST method's console test-invoke never produces a terminal answer when the target function's resource policy is scoped to a single stage.

AWS: recorded behavior only (the Smithy model describes TestInvokeMethod's request/response shape but no authorization semantics). AWS behaviour exercised by the check: TestInvokeMethod bypasses the method's AWS_IAM authorization but NOT the target function's resource policy. With a lambda:InvokeFunction grant whose AWS:SourceArn is scoped to `<rest-api-id>/beta/GET/<resource>` only, the test invoke is refused by Lambda and API Gateway reports status 500 for the test invocation, while a SigV4-signed GET against the deployed `beta` stage is served 200 with the item. After an additional grant is added for the test-invoke source ARN (`<rest-api-id>/test-invoke-stage/GET/<resource>`), the same TestInvokeMethod returns 200 with the item body.

Emulator: the check never reaches either verdict — the replay ends with `Error: Check timed out after 60.0s`, i.e. one of the calls in that sequence (test-invoke / signed stage invoke) never returns a terminal response within the harness budget, so neither the 500 refusal nor the post-grant 200 is ever observed.

Why: a client that uses the console/CLI test-invoke to validate a stage-scoped resource policy gets no answer at all. The distinction AWS makes — test-invoke is authorized by the function policy under the `test-invoke-stage` source ARN, not by the method's IAM authorizer — is unobservable, so callers cannot tell a correctly-refused invoke from a hung one, and tooling that polls for the status code blocks.

Fix: the TestInvokeMethod handler and the execute-api data-plane path it drives (resolving the integration, evaluating the Lambda resource policy for the `test-invoke-stage` source ARN, and returning a terminal status/body instead of never answering).

Traces: apigateway__dynamodb__iam__lambda.test-invoke-is-rejected-by-a-lambda-permission-scoped-to-the-beta-stage.

## Evidence
Replay verdict [mismatch] for check apigateway__dynamodb__iam__lambda.test-invoke-is-rejected-by-a-lambda-permission-scoped-to-the-beta-stage; detail: `Error: Check timed out after 60.0s`. Expected: TestInvokeMethod answers status 500 while the stage-scoped grant is the only grant, then 200 with the item after the test-invoke-stage grant is added. Actual: no terminal response within the 60s budget — nothing observable. Sibling checks of the same fixture group that exercise TestInvokeAuthorizer/TestInvokeMethod are refused with NotImplemented under already-filed issues; this check is not covered by any of them (the suite reports no owner for it).

## Steps to Reproduce
1. Create a REST API with a Lambda-proxy GET method and deploy it to stage `beta`.
2. Add a single lambda:AddPermission grant for apigateway.amazonaws.com with SourceArn `arn:aws:execute-api:<region>:<account>:<rest-api-id>/beta/GET/<resource>`.
3. Call TestInvokeMethod on that method: AWS answers 500 (the function policy does not admit the test-invoke source ARN).
4. Add a second grant with SourceArn `.../<rest-api-id>/test-invoke-stage/GET/<resource>` and call TestInvokeMethod again: AWS answers 200 with the item.
5. Against the emulator neither call reaches a terminal answer; the check is killed at the 60s harness budget.
