Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudcontrol.desired-state-pattern-violation-rejected
Issue-Shape-Id: com.amazonaws.cloudcontrol#CloudApiService
Issue-Key: cloudcontrol.desired-state-pattern-violation-rejected|run#9|AssertionError|assert read_code == "InvalidRequestException", (
Issue-Commit: 821434d5aa04daf94821fec33fa90277b7d33580

# cloudcontrol: GetResource on an AWS::Logs::LogGroup identifier the downstream read handler cannot use answers ResourceNotFoundException/NotFound instead of InvalidRequestException/InvalidRequest

> _Note: this issue was identified against the code base at commit `821434d5aa04daf94821fec33fa90277b7d33580`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResource

## Description
ONE net-new root cause in the cloudcontrol (Cloud Control API) service. Model citations are from services/service-com-amazonaws-cloudcontrol-cloudapiservice/smithy-model/com-amazonaws-cloudcontrol-cloudapiservice.json.

Cause 1 of 1: GetResource classifies EVERY unreadable identifier as absent, so an identifier the resource type's read handler cannot even submit downstream is reported ResourceNotFoundException (HandlerErrorCode NotFound) instead of InvalidRequestException (HandlerErrorCode InvalidRequest).
AWS: GetResource invokes the resource type's READ handler with the supplied Identifier and surfaces that handler's own diagnosis. For AWS::Logs::LogGroup the read handler forwards the identifier to CloudWatch Logs DescribeLogGroups as logGroupNamePrefix, and Logs enforces the same character class the type schema's pattern enforces on LogGroupName — a name containing a colon is not a legal prefix, so the downstream call is refused before any existence question is answered. The handler therefore returns status FAILED with HandlerErrorCode InvalidRequest, and Cloud Control raises error code InvalidRequestException (Smithy: GetResource models InvalidRequestException, errorKind client, httpError 400 — "The resource handler has returned that invalid input from the user has generated a generic exception"; ResourceNotFoundException is a separate modeled error, httpError 404, for a resource that is simply absent). The message is the handler-passthrough template: fixed text `<TypeName> Handler returned status FAILED: ` + the downstream service's own refusal text, which for this path names the rejected request member `logGroupNamePrefix`, + ` (HandlerErrorCode: InvalidRequest, RequestToken: <token>)`. The TypeName, the quoted downstream text and the RequestToken are per-request/per-type fill-ins; the `Handler returned status FAILED:` wording and the `(HandlerErrorCode: <code>, RequestToken: <id>)` suffix are the fixed text, and the requirement here is that <code> is InvalidRequest, not NotFound. A LEGAL identifier of the same type that was never created still reports absence (ResourceNotFoundException, `(HandlerErrorCode: NotFound, ...)`), which is what makes the two classes distinguishable.
Emulator: the read is refused with error code ResourceNotFoundException and the message `AWS::Logs::LogGroup Handler returned status FAILED: Resource of type 'AWS::Logs::LogGroup' with identifier '{"/properties/LogGroupName":"<name-with-colon>"}' was not found. (HandlerErrorCode: NotFound, RequestToken: <token>)` — i.e. the identifier is never submitted to the Logs read path at all; any identifier that does not match a stored resource is rendered as absence. (Collateral, same message: the identifier is rendered as the JSON primary-identifier object `{"/properties/LogGroupName":"..."}` rather than the bare log group name AWS quotes; that rendering defect is already filed for the SQS handler as 20261110T221719Z-cloudcontrol-the-aws-sqs-queue-handler-not-found.md and is not re-filed here.)
Why: a client branches on the error CODE to tell "this identifier is malformed, fix the caller" from "this resource does not exist, create it or stop polling". Told NotFound for a structurally impossible identifier, idempotent create-if-absent logic proceeds to create, reconciliation loops treat a permanent caller error as a transient absence and retry forever, and the HTTP status is 404 where AWS answers 400. The emulator also reads as more permissive than AWS: a schema violation that CreateResource correctly refuses becomes invisible on the read plane.
Fix: the GetResource handler's not-found classification (services/service-com-amazonaws-cloudcontrol-cloudapiservice/src/ops/get_resource.rs, with the per-type read path and the handler-result-to-error mapping it calls in src/state.rs) must run the resource type's read path for the identifier and map a downstream refusal of the identifier to HandlerErrorCode InvalidRequest / InvalidRequestException at HTTP 400, reserving NotFound/ResourceNotFoundException for an identifier the downstream read accepted and reported absent. This is NOT covered by the open 20261115T091557Z-cloudcontrol-5-net-new-causes-registry-schema-refusals.md (that issue is about front-door CreateResource/UpdateResource schema refusals and the JSON-identifier message; its replay keys do not include this check, and this check's create-side assertions pass).
Traces: cloudcontrol.desired-state-pattern-violation-rejected.

## Evidence
From the replay of suite simparity-behavioral-aws-v1 against the emulator. Generated log group names and request tokens in these quotes are per-run fill-ins, not requirements.

cloudcontrol.desired-state-pattern-violation-rejected (mismatch):
`AssertionError: GetResource for a pattern-violating identifier is refused, not reported absent; got ResourceNotFoundException: 'AWS::Logs::LogGroup Handler returned status FAILED: Resource of type \'AWS::Logs::LogGroup\' with identifier \'{"/properties/LogGroupName":"sim2real-behav-cc-loggroup-57e88ce85c41:colon-not-allowed"}\' was not found. (HandlerErrorCode: NotFound, RequestToken: f29978c2-a54f-4bfe-ab2c-8177d60e651d)'`

Expected (AWS): error code InvalidRequestException at HTTP 400, message containing the fixed fragments `AWS::Logs::LogGroup Handler returned status FAILED`, the rejected downstream member name `logGroupNamePrefix`, and `(HandlerErrorCode: InvalidRequest`.
Observed: error code ResourceNotFoundException at HTTP 404 with `(HandlerErrorCode: NotFound` and no mention of the downstream prefix member.

Control in the same check, which the emulator does satisfy and which proves the two classes must differ: GetResource on a LEGAL, never-created name of the same type answers ResourceNotFoundException with `(HandlerErrorCode: NotFound`. The create-side assertions in this check also pass (CreateResource refuses the colon-bearing LogGroupName with ValidationException / "Model validation failed (#/LogGroupName: failed validation constraint for keyword [pattern])"), so the surviving divergence is confined to the read plane.

## Steps to Reproduce
Boot the emulator; cc = client("cloudcontrol"). Let base = a unique legal log group name, illegal = base + ":colon-not-allowed".

1. cc.create_resource(TypeName="AWS::Logs::LogGroup", DesiredState=json.dumps({"LogGroupName": illegal, "RetentionInDays": 1})) — the request is refused with ValidationException "Model validation failed (#/LogGroupName: failed validation constraint for keyword [pattern])" (this part already matches AWS).
2. cc.get_resource(TypeName="AWS::Logs::LogGroup", Identifier=illegal) — EXPECT ClientError with Error.Code == "InvalidRequestException" (HTTP 400) whose message contains "AWS::Logs::LogGroup Handler returned status FAILED", "logGroupNamePrefix" and "(HandlerErrorCode: InvalidRequest". OBSERVED Error.Code == "ResourceNotFoundException" (HTTP 404) with "(HandlerErrorCode: NotFound" and the identifier rendered as {"/properties/LogGroupName": illegal}.
3. cc.get_resource(TypeName="AWS::Logs::LogGroup", Identifier=base + "-never-created") — expect, and the emulator gives, ResourceNotFoundException with "(HandlerErrorCode: NotFound": the contrast with step 2 is the requirement.
