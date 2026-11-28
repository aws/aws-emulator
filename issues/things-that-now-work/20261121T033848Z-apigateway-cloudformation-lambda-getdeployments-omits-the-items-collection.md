Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudformation__lambda.lambda-sdk-apigateway-create-deployment-missing-permission-negative
Issue-Key: apigateway__cloudformation__lambda.lambda-sdk-apigateway-create-deployment-missing-permission-negative|run#15|KeyError|not_deployed = apig.get_deployments(restApiId=api_without)["items"]
Issue-Commit: be68e03a3b164dfd55295662738d8b5c0a02662c

# apigateway__cloudformation__lambda: GetDeployments omits the `items` collection member for a REST API with no deployments, so the effect-absent read raises KeyError

> _Note: this issue was identified against the code base at commit `be68e03a3b164dfd55295662738d8b5c0a02662c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDeployments

## Description
One net-new root cause for this service heading. The failing check is marked neither ALREADY FILED nor OWNED by the suite report. RELATED, NOT COVERING: open issue 20261117T024849Z-apigateway-cognito-idp-lambda-getdeployments-omits-the-items.md describes the same GetDeployments defect but carries only the replay key of the `apigateway__cognito-idp__lambda` check, and open issue 20261114T023619Z-apigateway-nine-empty-collection-listings-omit-the-items.md names nine OTHER apigateway listings (GetAuthorizers, GetUsagePlans, GetApiKeys, ...) and not GetDeployments; neither carries this check's replay key, so this heading's check is still unowned. A fix to the shared empty-collection helper may close all of them at once; a fix scoped to the other operations will not.

Cause 1 of 1: a 200 GetDeployments listing of an EMPTY collection is served with the collection member ABSENT instead of present-and-empty.

AWS: Smithy (com.amazonaws.apigateway#BackplaneControlService) models GetDeployments as HTTP GET /restapis/{restApiId}/deployments, success code 200, output shape `Deployments` with members `items` (target ListOfDeployment, jsonName `item`) and `position`, and `paginated {inputToken: position, outputToken: position, items: items, pageSize: limit}`. The model does not state member presence, so for the empty case this is recorded behavior only: an AWS 200 listing always carries its collection member on the wire under the JSON key `item` — surfaced by SDKs as `items` — and a collection holding nothing answers an empty list, never an absent member. There is no error here: the status is 200 and no error code or message is involved.

Emulator: the 200 body omits `item` entirely, so the SDK-parsed response has no `items` key at all and the idiomatic read `apig.get_deployments(restApiId=api_id)["items"]` raises `KeyError: 'items'`. The check errors at `not_deployed = apig.get_deployments(restApiId=api_without)["items"]` with `KeyError: 'items'` — after the denial it was probing had already been observed correctly.

Why: the canonical effect-absent assertion for an IAM-denied control-plane call — "no deployment was actually created server-side" — is unwritable. A botocore client raises KeyError and a typed SDK gets a missing/None list, exactly on the API that owns nothing yet, so any client that lists before creating (pagination loops, "is it deployed?" probes, teardown sweeps) breaks on the empty case rather than iterating zero items.

Fix: the GetDeployments response path in the apigateway service crate and the shared empty-collection helper it uses when building the paged output — a 200 listing must report a present collection member, an empty list when there is nothing to report, not omit it.

Traces: apigateway__cloudformation__lambda.lambda-sdk-apigateway-create-deployment-missing-permission-negative.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator built from this checkout.

[error] apigateway__cloudformation__lambda.lambda-sdk-apigateway-create-deployment-missing-permission-negative
  check_apigateway__cloudformation__lambda_lambda_sdk_apigateway_create_deployment_missing_permission_negative.py:60
  `not_deployed = apig.get_deployments(restApiId=api_without)["items"]`
  -> KeyError: 'items'

Expected (AWS): HTTP 200 whose body carries the wire key `item`, so the parsed response contains `items` and `response["items"] == []` for a REST API that has no deployments.
Actual (emulator): HTTP 200 whose body omits `item` entirely, so the parsed response has no `items` member and the read raises KeyError.

Note on values: the REST API id, role name and function name in this recording are per-run values and are not part of the requirement. The requirement is structural — the 200 body of a listing always carries its collection member, including when the collection is empty.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and point a boto3 `apigateway` client at it.
2. `api_id = apigw.create_rest_api(name=...)["id"]`. Create NO deployment.
3. Call `apigw.get_deployments(restApiId=api_id)["items"]`.
   Observed: HTTP 200 with no `item` key in the body; the parsed response has no `items` member and the read raises `KeyError: 'items'`. Expected: `items == []`.
4. Full scenario: replay apigateway__cloudformation__lambda.lambda-sdk-apigateway-create-deployment-missing-permission-negative — it correctly observes the denied in-Lambda CreateDeployment, then dies on the GetDeployments read that proves the effect is absent, before its positive control runs.
