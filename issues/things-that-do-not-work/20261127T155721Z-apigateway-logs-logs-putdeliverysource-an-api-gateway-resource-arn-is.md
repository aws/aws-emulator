Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__logs.apigw-delivery-source-rest-api-arn-without-stage-rejected apigateway__logs.apigw-stage-delivery-source-access-log-type-rejected
Issue-Key: apigateway__logs.apigw-delivery-source-rest-api-arn-without-stage-rejected|_code_and_message#17|AssertionError|raise AssertionError(f"the call was accepted, expected a refusal: {answer!r}")
Issue-Key: apigateway__logs.apigw-stage-delivery-source-access-log-type-rejected|_refusal#13|AssertionError|raise AssertionError(f"expected a refusal, but the call succeeded: {got!r}")
Issue-Commit: 7300602cd0fc37394a6328fdd869591f4a36c031

# logs PutDeliverySource: an API Gateway resource ARN is not validated — a REST-API ARN without /stages/<stage> is accepted, and an ACCESS_LOGS-only stage ARN is accepted for EXECUTION_LOGS

> _Note: this issue was identified against the code base at commit `7300602cd0fc37394a6328fdd869591f4a36c031`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutDeliverySource

## Description
PutDeliverySource performs no service-specific validation of `resourceArn`/`logType` for `service=apigateway`, so two requests AWS refuses are accepted and a delivery source is created.

Cause 1 of 2 — a REST API ARN with no stage segment is accepted.
AWS: for API Gateway vended logs the delivery source must name a *stage*, i.e. `arn:aws:apigateway:<region>:<account>:/restapis/<api-id>/stages/<stage-name>`. A bare `arn:aws:apigateway:<region>:<account>:/restapis/<api-id>` is refused with a 400 ValidationException naming `resourceArn` as the member that failed; the fixed text is the validation sentence and the variable part is the ARN taken from the request. Smithy: `PutDeliverySource` models `ValidationException` and `resourceArn` is a required member; the per-service ARN grammar is documented behavior only (recorded behavior: the call is refused).
Emulator: the call returns HTTP 200 and creates the source — `{'deliverySource': {'name': '<name>', 'arn': 'arn:aws:logs:<region>:<account>:delivery-source:<name>', 'resourceArns': ['arn:aws:apigateway:<region>:<account>:/restapis/<api-id>'], 'service': 'apigateway', 'logType': 'EXECUTION_LOGS'}}`.
Traces: apigateway__logs.apigw-delivery-source-rest-api-arn-without-stage-rejected.

Cause 2 of 2 — a logType the named resource cannot vend is accepted.
AWS: the `logType` must be one the named API Gateway resource actually vends; a stage ARN registered for the wrong log type is refused with a 400 ValidationException naming `logType`. Recorded behavior only for the exact sentence.
Emulator: the call returns HTTP 200 with `{'resourceArns': ['arn:aws:apigateway:<region>:<account>:/restapis/<api-id>/stages/<stage>'], 'service': 'apigateway', 'logType': 'EXECUTION_LOGS'}`.
Traces: apigateway__logs.apigw-stage-delivery-source-access-log-type-rejected.

Why: a client that mis-spells the delivery-source ARN or asks for a log type the resource cannot vend gets a success and a delivery source that will never deliver. The error is then invisible until logs silently fail to arrive, and negative tests for delivery wiring pass vacuously.

Fix: the PutDeliverySource handler's validation — add the per-`service` `resourceArn` grammar and the allowed `logType` set for that resource kind, refusing with ValidationException before any state is written.

## Evidence
Cause 1 — expected: refusal (ValidationException, 400). Actual: HTTPStatusCode 200 and {'deliverySource': {..., 'resourceArns': ['arn:aws:apigateway:us-west-2:000000000000:/restapis/n0vaonf67p'], 'service': 'apigateway', 'logType': 'EXECUTION_LOGS'}}. Assertion: "the call was accepted, expected a refusal".
Cause 2 — expected: refusal. Actual: HTTPStatusCode 200 and {'deliverySource': {..., 'resourceArns': ['arn:aws:apigateway:us-west-2:000000000000:/restapis/9uno40o3fd/stages/edge'], 'logType': 'EXECUTION_LOGS'}}. Assertion: "expected a refusal, but the call succeeded".

## Steps to Reproduce
1. Create a REST API and a stage.
2. Call logs:PutDeliverySource with service='apigateway', logType='EXECUTION_LOGS' and resourceArn='arn:aws:apigateway:<region>:<account>:/restapis/<api-id>' (no /stages/<stage>) — observe HTTP 200 instead of a ValidationException.
3. Call logs:PutDeliverySource with a stage ARN and a logType that stage does not vend — observe HTTP 200 instead of a ValidationException.
