Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudformation.cfn-method-request-model-reference-rejected apigateway__cloudformation.cfn-resource-existence-apigateway-deployment apigateway__cloudformation.cfn-resource-existence-apigateway-method apigateway__cloudformation.cfn-resource-existence-apigateway-resource apigateway__cloudformation.deploy-apigateway-documentation-and-gateway-response-round-trip apigateway__cloudformation.deploy-apigateway-gateway-response-invalid-status-code-rejected apigateway__cloudformation.deploy-apigateway-usage-plan-invalid-quota-rejected apigateway__cloudformation.deploy-apigateway-usageplankey-invalid-keytype-rejected apigateway__cloudformation.deploy-apigateway-usageplankey-resource-round-trip apigateway__cloudformation.deploy-cdk-restapi-request-validator-model-rejects-invalid-body-test-invoke
Issue-Key: apigateway__cloudformation.cfn-method-request-model-reference-rejected|run#21|AssertionError|assert brk_status == "ROLLBACK_COMPLETE", (
Issue-Key: apigateway__cloudformation.cfn-resource-existence-apigateway-deployment|run#23|AssertionError|assert message == "Invalid Deployment identifier specified", (
Issue-Key: apigateway__cloudformation.cfn-resource-existence-apigateway-method|run#27|AssertionError|assert message == "Invalid Resource identifier specified", f"unexpected not-found message {message!r}"
Issue-Key: apigateway__cloudformation.cfn-resource-existence-apigateway-resource|run#25|AssertionError|assert message == "Invalid Resource identifier specified", f"unexpected not-found message {message!r}"
Issue-Key: apigateway__cloudformation.deploy-apigateway-documentation-and-gateway-response-round-trip|run#18|AssertionError|assert out["GatewayResponseRef"] == f"{api_id}:{_RESPONSE_TYPE}", (
Issue-Key: apigateway__cloudformation.deploy-apigateway-gateway-response-invalid-status-code-rejected|run#8|KeyError|got = (gr["statusCode"], gr["defaultResponse"])
Issue-Key: apigateway__cloudformation.deploy-apigateway-usage-plan-invalid-quota-rejected|run#11|AssertionError|assert stack_failed == [_STACK_REASON], (
Issue-Key: apigateway__cloudformation.deploy-apigateway-usageplankey-invalid-keytype-rejected|run#18|AssertionError|assert rows == [
Issue-Key: apigateway__cloudformation.deploy-apigateway-usageplankey-resource-round-trip|run#18|AssertionError|assert out["PlanKeyRef"] == want_ref, f"Ref {out['PlanKeyRef']!r} != documented {want_ref!r}"
Issue-Key: apigateway__cloudformation.deploy-cdk-restapi-request-validator-model-rejects-invalid-body-test-invoke|run#23|AssertionError|assert invalid["status"] == 400, (
Issue-Commit: 86c25e3173c48b1c4d4a9173324e183693bfe876

# apigateway__cloudformation: 6 net-new causes — a not-found refusal is worded wrong, Ref returns the wrong id for UsagePlanKey/GatewayResponse, template validation refusals carry no reasons, request-model validation is never enforced, and an invalid gateway-response status code is accepted

> _Note: this issue was identified against the code base at commit `86c25e3173c48b1c4d4a9173324e183693bfe876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description

LOCKFILE NOTE (added by the fixer, so the reason for the one dependency-resolution line in this
change is stated in the change itself, for a reader who is not shown `Cargo.lock`). This fix adds
NO dependency and changes NO manifest: no `Cargo.toml` of this workspace is touched, nothing is
vendored, and the only edit outside the apigateway and cloudformation crates and their tests is a
RESOLUTION edit to `Cargo.lock` that only REMOVES entries. Exactly three package entries go —
`malachite-base 0.15.0`, `malachite-bigint 0.15.0` and `malachite-nz 0.15.0` — and the one line
that named the last of them, `pymath`'s dependency on `malachite-bigint`, is pointed at the 0.12.0
entry the lockfile ALREADY carries and this change does not touch: `malachite-bigint 0.12.0`,
`checksum 47d673b71d4521ba5ca045cd23d62eda6b321474ff231d6a1698cc9b892851f2`, which the workspace's
own `python-guest-substrate` requires and `pymath`'s own `malachite-bigint@0` requirement admits.
Every other line that moves in that file is cargo dropping a version qualifier that is no longer
needed now that one version is locked (`"malachite-base 0.12.0"` becomes `"malachite-base"`),
which names that same entry. No entry is added, no version is raised, and nothing acquires a
capability: `malachite-bigint` is an in-process big-integer library (CC-05's first class — it
computes over data, opens no socket, spawns nothing, runs nothing at build time and carries no
FFI).

The reason for the edit: the 0.15.0 line a fresh resolution picks declares
`rust-version = 1.100.0`, which this toolchain (rustc 1.99.1) does not meet, so with 0.15.0 locked
`cargo build` refuses to run at all — "rustc 1.99.1 is not supported by the following packages:
malachite-base@0.15.0 requires rustc 1.100.0 …" — and the emulator cannot be built or booted.
Holding the one line is what lets the workspace compile here; the same one-line hold is recorded
in `issues/things-that-now-work/20261126T172228Z-service-quotas-listservices-catalog-omits-\
service-codes-and.md` for the same reason.

Six independent causes behind the 10 net-new failing apigateway__cloudformation checks (deploy-apigateway-client-certificate-resource-round-trip is excluded: it is already filed as an unimplemented GetClientCertificates stub).

Cause 1 of 6: the not-found refusal for a deleted API Gateway child resource is worded as an invalid-identifier error.
AWS: recorded behavior only — reading a Deployment, Method or Resource of an API that no longer exists is refused with API Gateway's not-found wording, naming the missing entity. The fixed text is API Gateway's own not-found message; the only variable part is the identifier taken from the request.
Emulator: all three reads answer with `Invalid API identifier specified <account-id>:<api-id>` — an invalid-identifier message built from the account id and the api id, not a not-found message.
Why: a client that distinguishes "this API is gone" (stop, treat as deleted) from "this identifier is malformed" (a bug in the caller) takes the wrong branch, and existence probes after a stack delete mis-report the cause.
Fix: the not-found path of the API Gateway Deployment/Method/Resource read handlers — the parent-API lookup miss must produce the not-found error, not the identifier-validation error.
Traces: apigateway__cloudformation.cfn-resource-existence-apigateway-deployment, apigateway__cloudformation.cfn-resource-existence-apigateway-method, apigateway__cloudformation.cfn-resource-existence-apigateway-resource

Cause 2 of 6: Ref on AWS::ApiGateway::UsagePlanKey and AWS::ApiGateway::GatewayResponse returns a synthesized CloudFormation name instead of the documented physical id.
AWS: recorded behavior only — Ref of AWS::ApiGateway::UsagePlanKey returns the composite `<key-id>:<usage-plan-id>`; Ref of AWS::ApiGateway::GatewayResponse returns the id API Gateway assigns, not a `<stack>-<logical-id>-<suffix>` string.
Emulator: Ref(UsagePlanKey) returns only the key id (`'<key-id>'` where the documented value is `'<key-id>:<usage-plan-id>'`), and Ref(GatewayResponse) returns the generated `<stack-name>-<logical-id>-<suffix>` physical name.
Why: a template that passes Ref of either resource to another resource or an Output hands downstream callers a value API Gateway cannot resolve, so cross-resource wiring and any client reading the output break.
Fix: the Ref return-value tables of the AWS::ApiGateway::UsagePlanKey and AWS::ApiGateway::GatewayResponse resource handlers.
Traces: apigateway__cloudformation.deploy-apigateway-usageplankey-resource-round-trip, apigateway__cloudformation.deploy-apigateway-documentation-and-gateway-response-round-trip

Cause 3 of 6: a Method naming a nonexistent RequestModel is provisioned instead of rolling the stack back.
AWS: recorded behavior only — an AWS::ApiGateway::Method whose RequestModels names a model the API does not declare fails to create and the stack settles ROLLBACK_COMPLETE.
Emulator: the stack settles 'CREATE_COMPLETE'.
Why: a broken API is deployed; the dangling model reference is never reported and the method's request validation can never work.
Fix: the AWS::ApiGateway::Method resource handler's create validation (resolve each RequestModels entry against the API's models) and the rollback it must drive.
Traces: apigateway__cloudformation.cfn-method-request-model-reference-rejected

Cause 4 of 6: a refused resource property produces a CREATE_FAILED/validation record with no reason at all.
AWS: recorded behavior only — when a resource property fails CloudFormation's own schema validation, the stack's CREATE_FAILED reason is exactly `Validation failed with 1 error(s). Call DescribeEvents to retrieve the full list of issues with resource and property details, resolve each error, then retry the operation.` (fixed text; only the error count varies), and the per-property detail is readable from the stack events.
Emulator: the CREATE_FAILED reasons list is empty (`[]`) for an out-of-range UsagePlan quota, and the validation rows are empty (`[]`) for an invalid UsagePlanKey KeyType.
Why: a client cannot tell why the stack failed — there is no reason to read, no property named, and no way to distinguish a schema violation from a throttle, a quota or a missing permission.
Fix: the resource-schema validation step in the stack-create path and the stack-event writer that must record its reason.
Traces: apigateway__cloudformation.deploy-apigateway-usage-plan-invalid-quota-rejected, apigateway__cloudformation.deploy-apigateway-usageplankey-invalid-keytype-rejected

Cause 5 of 6: an AWS::ApiGateway::GatewayResponse with an invalid StatusCode is accepted and its response carries no status code.
AWS: recorded behavior only — a GatewayResponse whose StatusCode is not a valid HTTP status is refused at create; the read-back of a valid one carries a statusCode member.
Emulator: the resource is created and the read-back response has no 'statusCode' key, so reading it raises KeyError: 'statusCode'.
Why: an invalid gateway response is installed on the API and clients reading the gateway-response configuration find no status code to apply.
Fix: the AWS::ApiGateway::GatewayResponse resource handler's StatusCode validation and the statusCode member of the GetGatewayResponse serializer.
Traces: apigateway__cloudformation.deploy-apigateway-gateway-response-invalid-status-code-rejected

Cause 6 of 6: a request validator with a body model does not reject a schema-invalid body.
AWS: recorded behavior only — a method with a RequestValidator that validates the request body answers 400 with API Gateway's validation message when the body does not satisfy the model's JSON schema.
Emulator: POST of a schema-invalid body answers 200 with the integration's own response body, so the validator is never applied.
Why: a client relying on API Gateway request validation to reject malformed input has no validation at all — invalid payloads reach the integration.
Fix: request-body validation in the execute-api invoke path (apply the method's RequestValidator and its RequestModels schema before the integration runs).
Traces: apigateway__cloudformation.deploy-cdk-restapi-request-validator-model-rejects-invalid-body-test-invoke

## Evidence
Cause 1 — AssertionError: unexpected not-found message 'Invalid API identifier specified <account-id>:<api-id>'  (3 checks). Expected API Gateway's not-found wording for the missing Deployment/Method/Resource.

Cause 2 — AssertionError: Ref '<key-id>' != documented '<key-id>:<usage-plan-id>'; AssertionError: Ref(GatewayResponse) '<stack-name>-GatewayResponse-<suffix>'.

Cause 3 — AssertionError: a Method naming a nonexistent model settled the stack at 'CREATE_COMPLETE', expected ROLLBACK_COMPLETE.

Cause 4 — AssertionError: the stack's CREATE_FAILED reasons were [], expected ['Validation failed with 1 error(s). Call DescribeEvents to retrieve the full list of issues with resource and property details, resolve each error, then retry the operation.']; AssertionError: unexpected validation rows: [].

Cause 5 — KeyError: 'statusCode' while reading the created gateway response (and the invalid StatusCode was accepted rather than refused).

Cause 6 — AssertionError: POST /items with a schema-invalid body returned status 200, expected 400: '{"message": "accepted"}'.

## Steps to Reproduce
1. Cause 1: deploy an AWS::ApiGateway::RestApi with a Resource/Method/Deployment, delete the stack, then call GetResource/GetMethod/GetDeployment with the recorded ids; compare the refusal message to API Gateway's not-found wording.
2. Cause 2: deploy AWS::ApiGateway::UsagePlanKey and AWS::ApiGateway::GatewayResponse and export Ref of each as a stack Output.
3. Cause 3: CreateStack with an AWS::ApiGateway::Method whose RequestModels names an undeclared model; expect ROLLBACK_COMPLETE.
4. Cause 4: CreateStack with an out-of-range AWS::ApiGateway::UsagePlan quota and with an invalid AWS::ApiGateway::UsagePlanKey KeyType; read DescribeStackEvents for the CREATE_FAILED reason.
5. Cause 5: CreateStack with an AWS::ApiGateway::GatewayResponse whose StatusCode is not a valid HTTP status; then read the gateway response back.
6. Cause 6: deploy a CDK RestApi with a request validator + body model and POST a body that violates the schema; expect 400.
7. Re-replay the 10 checks in trace_ids.
