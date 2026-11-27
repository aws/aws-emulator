Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.canary-deployment-without-stage-name-rejected apigateway.create-deployment-canary-settings-without-stage-name-rejected apigateway.create-deployment-canary-on-missing-stage-rejected apigateway.create-deployment-empty-stage-name-ignores-stage-settings apigateway.create-deployment-whitespace-stage-name-rejected apigateway.create-stage-empty-stage-name-rejected apigateway.create-deployment-method-without-integration-rejected
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway.canary-deployment-without-stage-name-rejected|_client_error#7|AssertionError|raise AssertionError(f"expected CreateDeployment to reject the stage-less canary, but it SUCCEEDED and returned {result!r}")
Issue-Key: apigateway.create-deployment-canary-settings-without-stage-name-rejected|run#8|AssertionError|assert message == _CANARY_NEEDS_STAGE, f"canary with stageName '' gave message {message!r}"
Issue-Key: apigateway.create-deployment-canary-on-missing-stage-rejected|_error#4|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: api-gateway|CreateDeployment|BadRequestException
Issue-Key: apigateway.create-stage-empty-stage-name-rejected|run#7|AssertionError|assert message == "Stage name must be non-empty", f"unexpected message {message!r}"
Issue-Key: apigateway.create-deployment-method-without-integration-rejected|_error#7|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Commit: 39b863d101414613449d16e44f0b23fd775a8d87

# apigateway: CreateDeployment/CreateStage preconditions and stage-name wording diverge (4 causes)

> _Note: this issue was identified against the code base at commit `39b863d101414613449d16e44f0b23fd775a8d87`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeployment

## Description
Four net-new root causes in the API Gateway control plane's deployment/stage creation path. Numbered; each carries its own failing check ids.

Cause 1 of 4: CreateDeployment does not enforce the canary stageName precondition.
AWS: Smithy apigateway CreateDeployment takes optional stageName and canarySettings (DeploymentCanarySettings) and models BadRequestException (httpError 400). A canary deployment is only meaningful against a stage, so AWS refuses a CreateDeployment that carries canarySettings with no stageName, or with stageName "", with 400 BadRequestException whose message is the fixed text "Invalid deployment content specified.Non null and non empty stageName must be provided for canary deployment. Provided value is " followed by the value actually supplied (absent/empty) — the fixed sentence is the requirement, the trailing value comes from the request.
Emulator: with canarySettings and no stageName the call SUCCEEDS, answering HTTP 201 with a Deployment ({id, createdDate}); with canarySettings and stageName "" it refuses 400 BadRequestException but with the emulator-authored message 'stageName must not be empty' instead of AWS's sentence.
Why: a client that relies on the refusal to catch a mis-wired canary deployment gets a deployment object back and believes a canary was configured, and a client matching on AWS's message never matches.
Fix: the CreateDeployment handler's canarySettings validation and its BadRequestException message text.
Traces: apigateway.canary-deployment-without-stage-name-rejected, apigateway.create-deployment-canary-settings-without-stage-name-rejected

Cause 2 of 4: CreateDeployment accepts canarySettings that name a stage which does not exist.
AWS: CreateDeployment models NotFoundException (httpError 404) and BadRequestException (400); a canary deployment whose stageName names no existing stage of the REST API is refused, not created.
Emulator: the call SUCCEEDS with HTTP 201 and a Deployment ({id, createdDate}); no refusal at all.
Why: a caller cannot distinguish "canary attached to the named stage" from "stage silently not found", and the canary never takes effect.
Fix: the CreateDeployment handler's canarySettings stage lookup/validation.
Traces: apigateway.create-deployment-canary-on-missing-stage-rejected

Cause 3 of 4: the shared stage-name validator both refuses a value AWS accepts and uses non-AWS wording.
AWS: on CreateDeployment, stageName is OPTIONAL and the empty string "" is accepted — the deployment is created with no stage and the stage-scoped settings (stageDescription, variables, cacheCluster*) are simply ignored. A blank/whitespace stageName (" ") IS refused, with 400 BadRequestException and the fixed message "Stage name must be non-empty", and the refused call must leave neither a deployment nor a stage behind. On CreateStage, stageName is required and an empty value is refused with the same fixed message "Stage name must be non-empty".
Emulator: every empty-or-blank stageName is refused with 400 BadRequestException carrying the emulator-authored message 'stageName must not be empty'. CreateDeployment(stageName="") is therefore refused where AWS creates the deployment, and both the CreateDeployment blank case and the CreateStage empty case carry the wrong message text.
Why: the common SDK/CDK idiom of creating a deployment with no stage fails outright, and clients matching AWS's "Stage name must be non-empty" never match.
Fix: the stage-name validation shared by the CreateDeployment and CreateStage handlers (accept "" on CreateDeployment; emit AWS's message).
Traces: apigateway.create-deployment-empty-stage-name-ignores-stage-settings, apigateway.create-deployment-whitespace-stage-name-rejected, apigateway.create-stage-empty-stage-name-rejected

Cause 4 of 4: CreateDeployment succeeds for a REST API whose method has no integration.
AWS: a deployment snapshots the API, and a method without an integration cannot be deployed; AWS refuses with 400 BadRequestException (modeled on CreateDeployment) naming the method that has no integration, and creates no deployment.
Emulator: the call SUCCEEDS with HTTP 201 and a Deployment ({id, createdDate}).
Why: the API deploys into a stage that cannot serve the method, so the failure surfaces much later at invoke time as a 5xx instead of at deploy time.
Fix: the CreateDeployment handler's pre-snapshot validation of every method's integration.
Traces: apigateway.create-deployment-method-without-integration-rejected

## Evidence
Cause 1: canary-deployment-without-stage-name-rejected — "expected CreateDeployment to reject the stage-less canary, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 201 ...}, 'id': ..., 'createdDate': ...}". create-deployment-canary-settings-without-stage-name-rejected — expected message prefix "Invalid deployment content specified.Non null and non empty stageName must be provided for canary deployment. Provided value is ", got 'stageName must not be empty'.
Cause 2: create-deployment-canary-on-missing-stage-rejected — "expected the call to be refused, but it succeeded: {... 'HTTPStatusCode': 201 ..., 'id': ..., 'createdDate': ...}".
Cause 3: create-deployment-empty-stage-name-ignores-stage-settings — "botocore.errorfactory.BadRequestException: An error occurred (BadRequestException) when calling the CreateDeployment operation: stageName must not be empty" where AWS returns 201 and ignores the stage settings. create-deployment-whitespace-stage-name-rejected — same emulator message; expected "Stage name must be non-empty". create-stage-empty-stage-name-rejected — "unexpected message 'stageName must not be empty'"; expected "Stage name must be non-empty".
Cause 4: create-deployment-method-without-integration-rejected — "expected the call to be rejected, but it SUCCEEDED and returned {... 'HTTPStatusCode': 201 ..., 'id': ..., 'createdDate': ...}".

## Steps to Reproduce
1. CreateRestApi, add a resource + method.
2. Cause 1: CreateDeployment(restApiId, canarySettings={'percentTraffic': 50}) with no stageName, then again with stageName=''. Expect 400 BadRequestException with AWS's "Invalid deployment content specified.Non null and non empty stageName must be provided for canary deployment. Provided value is ...".
3. Cause 2: CreateDeployment(restApiId, stageName='<a stage that does not exist>', canarySettings={...}). Expect a refusal, not 201.
4. Cause 3: CreateDeployment(restApiId, stageName='') — expect 201 and no stage; CreateDeployment(restApiId, stageName=' ') — expect 400 "Stage name must be non-empty" and no deployment/stage left behind; CreateStage(restApiId, stageName='', deploymentId=...) — expect 400 "Stage name must be non-empty".
5. Cause 4: PutMethod without a matching PutIntegration, then CreateDeployment. Expect 400 BadRequestException naming the method with no integration.
