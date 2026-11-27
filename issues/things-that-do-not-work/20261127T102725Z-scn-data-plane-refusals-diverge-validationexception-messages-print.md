Issue-Source: trace-sweep-divergence
Issue-Trace: flow-execution-identifier-validation
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: flow-execution-identifier-validation send-event-request-validation
Issue-Shape-Id: com.amazonaws.supplychain#GalaxyPublicAPIGateway
Issue-Key: scn|GetDataIntegrationFlowExecution|4xx->4xx|$.message
Issue-Key: scn|SendDataIntegrationEvent|4xx->4xx|$.message
Issue-Key: scn|GetDataIntegrationFlowExecution|4xx->4xx|$.Message
Issue-Key: scn|GetDataIntegrationFlowExecution|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: scn|GetDataIntegrationFlowExecution|4xx->4xx|status
Issue-Commit: e8f09314d35987b191cae688f7fba7f207639dbf

# scn data-plane refusals diverge: ValidationException messages print the anchored @pattern and instance authorization is skipped

> _Note: this issue was identified against the code base at commit `e8f09314d35987b191cae688f7fba7f207639dbf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDataIntegrationFlowExecution

## Description
THREE (3) net-new divergence clusters on AWS Supply Chain's data-plane API (service token `scn`, sdkId SupplyChain, endpointPrefix/sigv4Name `scn`, crate services/service-com-amazonaws-supplychain-galaxypublicapigateway, restJson1). All three are refusal responses on the `/api-data/data-integration/...` surface: the emulator renders the service's constraint-violation message with the model's anchored regex, and it validates a path label before it performs the instance authorization check that AWS performs first. A fixer MUST close all THREE causes before submitting.

Related open issue (does NOT cover these clusters): `20261029T180412Z-scn-refusal-messages-are-emulator-authored-prose-instead.md` owns the CreateDataLakeNamespace `name` cluster (supplychain-instance-error-paths#task#7) and the GetInstance not-found text; Causes 1 and 2 below are the SAME formatter fault on different operations/members, reported here as their own clusters because that issue's replay key does not cover them.

Cause 1 of 3: GetDataIntegrationFlowExecution's ValidationException message prints the executionId pattern anchored (fixture `flow-execution-identifier-validation`, step `task#2`; the cluster also covers step `task#4`; operation GetDataIntegrationFlowExecution). AWS: Smithy GetDataIntegrationFlowExecution is GET /api-data/data-integration/instance/{instanceId}/data-integration-flows/{flowName}/executions/{executionId}; member `executionId` targets UUID with @httpLabel, @required, @length 36..36 and @pattern `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`, and the operation declares the service-common `ValidationException` (@httpError 400, single `message` member, doc "The input does not satisfy the constraints specified by an AWS service."). The recording shows AWS answering 400 with the standard constraint template, whose FIXED TEXT is `1 validation error detected: Value at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>` — its only variable parts are the member name taken from the request's URI binding and the pattern taken from the model, and AWS prints that pattern UNANCHORED (`[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}`), without the `^`/`$` the @pattern trait carries. Emulator: 400 ValidationException (status and error class correct) whose message is identical except that it echoes the model's anchored spelling `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`. Why: the validation message is how a caller learns which member was wrong and what shape is accepted; a message carrying the emulator's internal regex spelling fails any exact-match assertion on AWS's template and leaks an anchored pattern the service never publishes. Fix: the ValidationException message formatter shared by this service's input validation (crate services/service-com-amazonaws-supplychain-galaxypublicapigateway, src/ops/) must strip leading `^` and trailing `$` when rendering a member's @pattern into the message.

Cause 2 of 3: SendDataIntegrationEvent's ValidationException message prints the instanceId pattern anchored (fixture `send-event-request-validation`, step `task#5`, operation SendDataIntegrationEvent). AWS: Smithy SendDataIntegrationEvent is POST /api-data/data-integration/instance/{instanceId}/data-integration-events; member `instanceId` targets UUID with @httpLabel, @required, @length 36..36 and @pattern `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`; the operation declares ValidationException @httpError 400. The recording shows the same constraint template with the member name `instanceId` and the pattern printed UNANCHORED. Emulator: 400 ValidationException with the anchored pattern, exactly as in Cause 1. Why: same as Cause 1 — the published constraint a client is told to satisfy differs from AWS's. Fix: the same shared message formatter; this cause confirms the fault is in the formatter and not in one operation's handler, so one edit must cover every member whose @pattern is rendered into a message.

Cause 3 of 3: GetDataIntegrationFlowExecution refuses an unknown instanceId with 400 ValidationException about `flowName` instead of AWS's 403 AccessDeniedException about the instance (fixture `flow-execution-identifier-validation`, step `task#3`, operation GetDataIntegrationFlowExecution). AWS: the operation declares `AccessDeniedException` (@httpError 403, doc "You do not have the required privileges to perform this action."). The recording shows that when `instanceId` is a well-formed UUID that names no instance of the calling account, the service answers 403 and does NOT evaluate the other path labels first — the response body carries the member serialized as `Message` (capital M, as recorded, not the model's lowercase `message`) with the text `Authorization Failed (Public). The instanceId <instanceId> does not exist, or does not belong to the account <accountId>`, plus an x-amzn-errortype header naming the error shape. In that message the FIXED TEXT is `Authorization Failed (Public). The instanceId ` + ` does not exist, or does not belong to the account `; the variable parts are the instanceId taken from the request URI and the 12-digit account id of the caller (the recording's concrete UUID and `000000000000` merely fill them). Emulator: 400 with x-amzn-errortype `ValidationException` and body `{"message": "1 validation error detected: Value at 'flowName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[A-Za-z0-9-]+$"}` — it validates the flowName @pattern (`^[A-Za-z0-9-]+$`) and never reaches the instance authorization check, so the status class, the error shape, the body member name and the message are all wrong. Why: a client branches on the error class — 403 AccessDeniedException means the instance is absent or not the caller's and the caller must fix credentials/instance id, while 400 ValidationException means a malformed request. The emulator sends callers to fix a flow name when the real fault is an instance they cannot access, and a client reading `Message` (as AWS serializes this refusal) finds no such member at all. Fix: the request-gating order in the data-plane flow-execution handlers (crate services/service-com-amazonaws-supplychain-galaxypublicapigateway, src/ops/) — resolve and authorize the `instanceId` BEFORE validating the remaining @httpLabel members, and emit AccessDeniedException with the recorded message template and the `Message` body key.

Traces: flow-execution-identifier-validation#task#2 (and #task#4), send-event-request-validation#task#5, flow-execution-identifier-validation#task#3.

## Evidence
Replay summary: trace_set_id=simparity-wire-aws-v1, emulator_up=true, bootstrap_signal=false, steps_total=114, passed=106, mismatched=8, diverged=6, divergence_count=5, score_pct=93.0. None of the three clusters below carried a `stub_path` or an `owned_by` owner.

Cause 1 — flow-execution-identifier-validation#task#2, GetDataIntegrationFlowExecution, expected 400 / actual 400 (status and error class MATCH; cluster also affects flow-execution-identifier-validation#task#4):
  $.message: value_mismatch
    expected: "1 validation error detected: Value at 'executionId' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}"
    actual:   "1 validation error detected: Value at 'executionId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$"
  Fixed text: "1 validation error detected: Value at '" + "' failed to satisfy constraint: Member must satisfy regular expression pattern: ". Variable parts: the member name (executionId, from the URI binding) and the pattern (from the model, rendered WITHOUT anchors). Sole deviation: the emulator's `^`/`$`.

Cause 2 — send-event-request-validation#task#5, SendDataIntegrationEvent, expected 400 / actual 400 (status and error class MATCH):
  $.message: value_mismatch
    expected: "1 validation error detected: Value at 'instanceId' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}"
    actual:   "1 validation error detected: Value at 'instanceId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$"

Cause 3 — flow-execution-identifier-validation#task#3, GetDataIntegrationFlowExecution, expected 403 / actual 400:
  status: expected 403, got 400
  $.Message: value_mismatch (expected 'Authorization Failed (Public). The instanceId 5c0e7a11-0000-4000-8000-0187797b3f19 does not exist, or does not belong to the account 000000000000', got "1 validation error detected: Value at 'flowName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[A-Za-z0-9-]+$")
  $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ValidationException')
  The quoted UUID and account id are the recording's values filling the template's two slots; the fixed text is everything else.

Model evidence (services/service-com-amazonaws-supplychain-galaxypublicapigateway/smithy-model/com-amazonaws-supplychain-galaxypublicapigateway.json, service GalaxyPublicAPIGateway, restJson1, awsService{sdkId SupplyChain, endpointPrefix scn}):
- GetDataIntegrationFlowExecution: GET /api-data/data-integration/instance/{instanceId}/data-integration-flows/{flowName}/executions/{executionId}; instanceId & executionId = UUID @httpLabel @required @length 36..36 @pattern ^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$; flowName = DataIntegrationFlowName @httpLabel @required @length 1..256 @pattern ^[A-Za-z0-9-]+$; errors include AccessDeniedException (403), ValidationException (400), ResourceNotFoundException (404).
- SendDataIntegrationEvent: POST /api-data/data-integration/instance/{instanceId}/data-integration-events; instanceId = UUID @httpLabel @required @pattern as above; errors include ValidationException (400), AccessDeniedException (403).
- AccessDeniedException: errorKind client, httpError 403, members {message: String}; ValidationException: errorKind client, httpError 400, members {message: String}. The recorded 403 serializes the member as `Message`.

## Steps to Reproduce
1. git clone the mockforge-aws-emulator repo for this work item and boot the emulator built from that checkout.
2. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus).
3. Cause 1: replay fixture `flow-execution-identifier-validation` step `task#2` — GET /api-data/data-integration/instance/<valid-uuid>/data-integration-flows/<valid-flow-name>/executions/<malformed-execution-id>. Observe 400 ValidationException whose message prints the pattern as ^[a-f0-9]{8}-...$; AWS's recorded message prints it as [a-f0-9]{8}-... (no anchors). Step `task#4` of the same fixture reproduces it.
4. Cause 2: replay fixture `send-event-request-validation` step `task#5` — POST /api-data/data-integration/instance/<malformed-instance-id>/data-integration-events. Same anchored-pattern message on member `instanceId`.
5. Cause 3: replay fixture `flow-execution-identifier-validation` step `task#3` — GET the flow-execution path with a well-formed instanceId that names no instance of the calling account (and a flowName that violates ^[A-Za-z0-9-]+$). Observe 400 with x-amzn-errortype: ValidationException and a body complaining about `flowName`; AWS answers 403 with x-amzn-errortype naming AccessDeniedException and body {"Message": "Authorization Failed (Public). The instanceId <instanceId> does not exist, or does not belong to the account <accountId>"}.
6. Diff each response's status, x-amzn-errortype header and body member against the fixture's expected response.
