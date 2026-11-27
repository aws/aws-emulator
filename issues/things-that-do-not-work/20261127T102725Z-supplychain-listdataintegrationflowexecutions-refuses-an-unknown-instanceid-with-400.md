Issue-Source: trace-sweep-divergence
Issue-Trace: supplychain-flow-executions-request-validation
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: supplychain-flow-executions-request-validation
Issue-Shape-Id: com.amazonaws.supplychain#GalaxyPublicAPIGateway
Issue-Key: supplychain|ListDataIntegrationFlowExecutions|4xx->4xx|$.Message
Issue-Key: supplychain|ListDataIntegrationFlowExecutions|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: supplychain|ListDataIntegrationFlowExecutions|4xx->4xx|status
Issue-Commit: e8f09314d35987b191cae688f7fba7f207639dbf

# supplychain ListDataIntegrationFlowExecutions refuses an unknown instanceId with 400 ValidationException instead of AWS's 403 AccessDeniedException

> _Note: this issue was identified against the code base at commit `e8f09314d35987b191cae688f7fba7f207639dbf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListDataIntegrationFlowExecutions

## Description
ONE (1) net-new divergence cluster reported under service token `supplychain` (AWS Supply Chain, sdkId SupplyChain, crate services/service-com-amazonaws-supplychain-galaxypublicapigateway, restJson1).

Cause 1 of 1: ListDataIntegrationFlowExecutions does not authorize the instance before validating the remaining path labels (fixture `supplychain-flow-executions-request-validation`, step `task#3`, operation ListDataIntegrationFlowExecutions). AWS: Smithy ListDataIntegrationFlowExecutions is GET /api-data/data-integration/instance/{instanceId}/data-integration-flows/{flowName}/executions, with `instanceId` (UUID, @httpLabel, @required, @length 36..36, @pattern ^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$) and `flowName` (DataIntegrationFlowName, @httpLabel, @required, @length 1..256, @pattern ^[A-Za-z0-9-]+$); it declares `AccessDeniedException` @httpError 403 (doc "You do not have the required privileges to perform this action.") alongside ValidationException @httpError 400. The recording shows that when `instanceId` is a well-formed UUID naming no instance of the calling account, the service answers 403 FIRST and never reports the other labels: the body carries the member serialized as `Message` (capital M, as recorded, not the model's lowercase `message`) with the text `Authorization Failed (Public). The instanceId <instanceId> does not exist, or does not belong to the account <accountId>`, plus an x-amzn-errortype header naming the error shape. FIXED TEXT: `Authorization Failed (Public). The instanceId ` + ` does not exist, or does not belong to the account `; VARIABLE parts: the instanceId taken from the request URI and the caller's 12-digit account id — the recording's concrete UUID and `000000000000` only fill those slots. Emulator: 400 with x-amzn-errortype `ValidationException` and body `{"message": "1 validation error detected: Value at 'flowName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[A-Za-z0-9-]+$"}` — wrong status class, wrong error shape, wrong body member name, and a message about a different member (it also prints the model's anchored pattern, which AWS renders unanchored). Why: clients branch on this distinction — 403 AccessDeniedException tells the caller the Supply Chain instance is absent or not theirs (fix credentials or instance id, surface a permission error), whereas 400 ValidationException tells it the request is malformed and sends it to correct a flow name that was never the problem; a client reading the `Message` member AWS serializes for this refusal finds no such member in the emulator's body. Fix: the request-gating order of the flow-executions list handler (crate services/service-com-amazonaws-supplychain-galaxypublicapigateway, src/ops/) — resolve and authorize `instanceId` BEFORE validating the remaining @httpLabel members, and emit AccessDeniedException with the recorded message template under the `Message` body key. The same ordering fault on GetDataIntegrationFlowExecution is filed under the `scn` service token (bundle titled "scn data-plane refusals diverge: ValidationException messages print the anchored @pattern and instance authorization is skipped"), because an issue carries a single service token; the anchored-pattern message rendering is also filed there and, for CreateDataLakeNamespace, in the open issue 20261029T180412Z-scn-refusal-messages-are-emulator-authored-prose-instead.md.

Traces: supplychain-flow-executions-request-validation#task#3.

## Evidence
Replay summary: trace_set_id=simparity-wire-aws-v1, emulator_up=true, bootstrap_signal=false, steps_total=114, passed=106, mismatched=8, diverged=6, divergence_count=5, score_pct=93.0. The cluster carried no `stub_path` and no `owned_by` owner.

supplychain-flow-executions-request-validation#task#3, ListDataIntegrationFlowExecutions, expected 403 / actual 400 (divergence_key service=supplychain|ListDataIntegrationFlowExecutions|status=403->400|keys=$.Message,$headers.x-amzn-errortype,status):
  status: expected 403, got 400
  $.Message: value_mismatch (expected 'Authorization Failed (Public). The instanceId 5e7a0c1d-9b2f-4c3e-8d4a-e70813ecadd4 does not exist, or does not belong to the account 000000000000', got "1 validation error detected: Value at 'flowName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[A-Za-z0-9-]+$")
  $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ValidationException')
  The quoted UUID and account id are this recording's values filling the message template's two slots; everything else in the sentence is fixed text.

Model evidence (services/service-com-amazonaws-supplychain-galaxypublicapigateway/smithy-model/com-amazonaws-supplychain-galaxypublicapigateway.json):
- ListDataIntegrationFlowExecutions: GET /api-data/data-integration/instance/{instanceId}/data-integration-flows/{flowName}/executions, code 200, paginated(nextToken/maxResults default 10 range 1..20); instanceId UUID @httpLabel @required @pattern ^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$; flowName @httpLabel @required @pattern ^[A-Za-z0-9-]+$.
- Errors: AccessDeniedException (client, 403), ValidationException (client, 400), ResourceNotFoundException (client, 404), ConflictException (409), ServiceQuotaExceededException (402), ThrottlingException (429), InternalServerException (500).
- AccessDeniedException: members {message: String}, httpError 403; the recorded refusal serializes it as `Message`.

## Steps to Reproduce
1. git clone the mockforge-aws-emulator repo for this work item and boot the emulator built from that checkout.
2. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus).
3. Replay fixture `supplychain-flow-executions-request-validation` step `task#3` — GET /api-data/data-integration/instance/<well-formed-uuid-that-names-no-instance>/data-integration-flows/<flow-name-violating-^[A-Za-z0-9-]+$>/executions.
4. Observe the emulator answer 400 with header x-amzn-errortype: ValidationException and body {"message": "1 validation error detected: Value at 'flowName' ..."}.
5. Compare with the fixture's recorded response: 403, x-amzn-errortype naming AccessDeniedException, body {"Message": "Authorization Failed (Public). The instanceId <instanceId> does not exist, or does not belong to the account <accountId>"}.
6. Diff status, the x-amzn-errortype header and the body member name/text against the recording.
