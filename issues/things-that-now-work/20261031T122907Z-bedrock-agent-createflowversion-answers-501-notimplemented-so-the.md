Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agent.flow-version-creation-without-prepare-rejected
Issue-Shape-Id: com.amazonaws.bedrockagent#AmazonBedrockAgentBuildTimeLambda
Issue-Key: bedrock-agent.flow-version-creation-without-prepare-rejected|run#10|AssertionError|assert code == "ConflictException", f"expected ConflictException, got {code}"
Issue-Commit: 32f27f5adb5ef4f1260f24418d9e3155f5d658ad

# bedrock-agent CreateFlowVersion answers 501 NotImplemented, so the NotPrepared ConflictException ordering guard never happens

> _Note: this issue was identified against the code base at commit `32f27f5adb5ef4f1260f24418d9e3155f5d658ad`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFlowVersion

## Description
One net-new root cause in the bedrock-agent service.

Cause 1 of 1: `CreateFlowVersion` is still an unimplemented scaffold stub, so it answers HTTP 501 `NotImplemented` for every request — including the one AWS refuses with 409 `ConflictException` because the flow's DRAFT has never been Prepared. Failing check: bedrock-agent.flow-version-creation-without-prepare-rejected.

AWS: `CreateFlowVersion` (POST /flows/{flowIdentifier}/versions, success 201) creates the next incrementally numbered version of a flow. Smithy: CreateFlowVersion input `flowIdentifier` (FlowIdentifier, @httpLabel, required, pattern ^(arn:aws:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:flow/[0-9a-zA-Z]{10})|([0-9a-zA-Z]{10})$), optional `description` (len 1..200) and `clientToken` (@idempotencyToken, len 33..256); output requires `name`, `executionRoleArn`, `id`, `arn`, `status` (FlowStatus: Failed | NotPrepared | Prepared | Preparing), `createdAt` (date-time) and `version` (NumericalVersion, pattern ^[0-9]{1,5}$ — "Versions are numbered incrementally, starting from 1"). Modeled errors include `ConflictException` (HTTP 409, "There was a conflict performing an operation") and `ResourceNotFoundException` (404). Versioning is ordered behind preparation: a flow whose DRAFT is in status `NotPrepared` cannot be versioned, and AWS refuses the call with 409 `ConflictException` and the message (recorded behavior; fixed text, with one variable part):
  "Couldn't perform CreateFlowVersion operation. The resource that you specified is currently in status NotPrepared. Retry your request after the resource changes to a valid status."
Fixed text: "Couldn't perform " + the operation name + " operation. The resource that you specified is currently in status " + <status> + ". Retry your request after the resource changes to a valid status." The ONLY variable parts are the operation name (`CreateFlowVersion`) and <status>, which is the flow's current `FlowStatus` read from state (here `NotPrepared`) — never a literal from one run. The rejected call must also have NO effect: ListFlowVersions must be byte-identical before and after, i.e. no numbered version may leak. Once the flow is Prepared the identical call succeeds with 201 and a numeric `version`, and exactly one new version appears in ListFlowVersions.

Emulator: every `CreateFlowVersion` call is refused with HTTP 501 and error code `NotImplemented`, message "operation CreateFlowVersion is scaffolded but not implemented in this emulator". The handler body in services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda/src/ops/create_flow_version.rs returns `Err(crate::scaffolded::Scaffolded::not_implemented("CreateFlowVersion"))` unconditionally and the file still carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker. The replayed check asserts the error code on the un-prepared DRAFT and gets `NotImplemented` where `ConflictException` is required (AssertionError: expected ConflictException, got NotImplemented).

Why: a client gets a 501 — a server-side "not written yet" signal — for a modeled, routable control-plane operation. Error-code branching breaks: SDK retry/classification treats 501 as a non-retryable server fault rather than the 409 conflict that tells the caller to call PrepareFlow first and retry, so flow-deployment orchestration (prepare -> version -> alias -> invoke) cannot be driven at all, and no caller can ever obtain a numbered flow version. Negative-path clients that expect ConflictException on an unprepared flow see a 5xx and will surface an emulator defect as an AWS outage.

Fix: implement the operation in its own handler, services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda/src/ops/create_flow_version.rs (and delete the SCAFFOLDED marker line). It needs: a state lookup of the flow by `flowIdentifier` (id or ARN form) returning `ResourceNotFoundException` when absent; a pre-condition check on the DRAFT's `status` that returns the modeled `ConflictException` with the message template above whenever the status is not `Prepared`, taking the status token from state and refusing before any mutation so no version is recorded; and, for a `Prepared` flow, allocation of the next incremental numeric version in the service state plus a 201 response carrying the required output members (id, arn, name, executionRoleArn, status, createdAt, version, and definition/description when set) so the new version is visible to ListFlowVersions/GetFlowVersion. `clientToken` idempotency belongs in the same handler.

Traces: bedrock-agent.flow-version-creation-without-prepare-rejected (mismatch; assertion at check_bedrock_agent_flow_version_creation_without_prepare_rejected.py line 39). Note this check also exercises PrepareFlow later in its positive control, which is covered separately by the open issue 20261029T171944Z-bedrock-agent-prepareflow-stub.md; that issue does not name CreateFlowVersion and does not cover this refusal.

## Evidence
Expected (AWS): CreateFlowVersion on a flow whose status is NotPrepared -> HTTP 409, Error.Code = "ConflictException", Error.Message = "Couldn't perform CreateFlowVersion operation. The resource that you specified is currently in status NotPrepared. Retry your request after the resource changes to a valid status." and no new entry in ListFlowVersions. After PrepareFlow succeeds, the identical call -> HTTP 201 with a numeric `version` member.

Actual (emulator): HTTP 501, Error.Code = "NotImplemented", Error.Message = "operation CreateFlowVersion is scaffolded but not implemented in this emulator" — for every CreateFlowVersion call, prepared or not.

Replay output: bedrock-agent.flow-version-creation-without-prepare-rejected [mismatch]
  File "check_bedrock_agent_flow_version_creation_without_prepare_rejected.py", line 39, in run
    assert code == "ConflictException", f"expected ConflictException, got {code}"
  AssertionError: expected ConflictException, got NotImplemented

Source: services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda/src/ops/create_flow_version.rs line 4 (`//! SCAFFOLDED, NOT IMPLEMENTED:`) and line 197 (`Err(crate::scaffolded::Scaffolded::not_implemented("CreateFlowVersion"))`). The modeled errors at lines 124-132 already include ConflictException and ResourceNotFoundException.

## Steps to Reproduce
1. Create an IAM role Bedrock flows can assume (trust principal bedrock.amazonaws.com).
2. bedrock-agent CreateFlow with a minimal passthrough definition (Input node -> Output node, one Data connection) and that executionRoleArn; note the returned flow id.
3. bedrock-agent GetFlow on that id and confirm status == "NotPrepared".
4. bedrock-agent ListFlowVersions on that id and record the summaries.
5. bedrock-agent CreateFlowVersion(flowIdentifier=<id>) on the un-prepared DRAFT.
   Observed: HTTP 501 NotImplemented. Expected: HTTP 409 ConflictException with the status-based message above.
6. ListFlowVersions again and confirm the listing is unchanged (no leaked version).
7. PrepareFlow, poll GetFlow until status == "Prepared", then repeat CreateFlowVersion.
   Expected: HTTP 201 with a numeric `version`, and exactly one new entry in ListFlowVersions.
