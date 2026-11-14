Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agent-flow-error-paths
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agent-flow-error-paths
Issue-Shape-Id: com.amazonaws.bedrockagent#AmazonBedrockAgentBuildTimeLambda
Issue-Key: bedrock-agent|CreateFlowVersion|4xx->2xx|$.message
Issue-Key: bedrock-agent|CreateFlowVersion|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: bedrock-agent|CreateFlowVersion|4xx->2xx|status
Issue-Commit: a52cf8398fdb2537e1d726dcb152539e10f26618

# bedrock-agent CreateFlowVersion has no prepare-ordering guard: a NotPrepared flow is versioned with 201 instead of refused 409 ConflictException

> _Note: this issue was identified against the code base at commit `a52cf8398fdb2537e1d726dcb152539e10f26618`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFlowVersion

## Description
1 cause.

Cause 1 of 1: CreateFlowVersion on a flow whose DRAFT has never been Prepared is accepted and cuts a numbered version, instead of being refused with the modeled 409 ConflictException (fixture bedrock-agent-flow-error-paths, step task#4).

AWS: Smithy (services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda/smithy-model/com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda.json) models CreateFlowVersion as POST /flows/{flowIdentifier}/versions with success status 201, input flowIdentifier (FlowIdentifier, @httpLabel, @required), optional description (@length 1..200) and clientToken (@idempotencyToken, @length 33..256), and output requiring name, executionRoleArn, id, arn, status (FlowStatus enum = Failed | Prepared | Preparing | NotPrepared), createdAt and version (NumericalVersion, @pattern ^[0-9]{1,5}$ — "Versions are numbered incrementally, starting from 1"). Its modeled errors include ConflictException (@error client, @httpError 409, doc "There was a conflict performing an operation"). Versioning is ordered behind preparation: a flow that is not Prepared cannot be versioned, and AWS refuses the call with HTTP 409, error code ConflictException and the message (recorded behavior for this error class):
  "Couldn't perform CreateFlowVersion operation. The resource that you specified is currently in status NotPrepared. Retry your request after the resource changes to a valid status."
Fixed text of the template: "Couldn't perform " + <operation name> + " operation. The resource that you specified is currently in status " + <status> + ". Retry your request after the resource changes to a valid status." The ONLY variable parts are the operation name (here CreateFlowVersion) and <status>, which is the addressed flow's current FlowStatus token read from service state (NotPrepared in this recording) — neither is a literal to hard-code from one run. The refusal must also be side-effect-free: no version number may be consumed and nothing new may appear in ListFlowVersions.

Emulator: the request is accepted. Replay observed HTTP 201 where 409 was required; the response body carries the CreateFlowVersionResponse shape, and the asserted error members are absent — $.message missing (expected the sentence above, got None) and $headers.x-amzn-errortype missing (expected a shaped error type, got None). The handler never inspects the flow's status: services/.../src/ops/create_flow_version.rs resolves the identifier, replays clientToken, computes the next version and the per-flow quota, looks the flow up (ResourceNotFoundException when absent), then snapshots the flow into state unconditionally. CreateFlowVersionErrorModeled::ConflictException is declared in the error enum but is never constructed anywhere in the handler, so no status pre-condition exists.

Why: the prepare -> version -> alias -> invoke ordering that Bedrock flows depend on is unenforced. A client that versions a flow before PrepareFlow has driven it to Prepared gets a success and a numeric version pointing at an unprepared, unusable definition, instead of the 409 that tells it to prepare and retry; negative-path code branching on ConflictException never fires, and the leaked version is visible to ListFlowVersions/GetFlowVersion, so subsequent version numbering and quota accounting are off by the versions AWS would never have cut.

Fix: a pre-condition check in the CreateFlowVersion handler (services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda/src/ops/create_flow_version.rs) — after the flow lookup and before any state mutation, read the flow's FlowStatus from state and return the modeled ConflictException with the message template above whenever it is not Prepared, so the call is refused without consuming a version number or writing a FlowVersionRecord.

Related open issues that do NOT cover this divergence: 20261031T122907Z-bedrock-agent-createflowversion-answers-501-notimplemented-so-the.md and 20261102T104226Z-bedrock-agent-createflowversion-stub.md both describe CreateFlowVersion as an unimplemented 501 scaffold stub; the operation is now implemented and answers 201, and neither issue carries this fixture step's replay key. 20261029T172151Z-bedrock-agent-prepareflow-leaves-the-flow-in-notprepared.md covers PrepareFlow's own state transition, not this missing guard.

Traces: bedrock-agent-flow-error-paths#task#4.

## Evidence
Replay of trace set simparity-wire-aws-v1, fixture bedrock-agent-flow-error-paths, step task#4, operation CreateFlowVersion:
  status: expected 409, got 201
  $.message: missing (expected "Couldn't perform CreateFlowVersion operation. The resource that you specified is currently in status NotPrepared. Retry your request after the resource changes to a valid status.", got None)
  $headers.x-amzn-errortype: missing (expected a shaped error type (ConflictException), got None)

Expected (AWS): HTTP 409, x-amzn-errortype: ConflictException, message as above — with <operation name> = CreateFlowVersion and <status> = the flow's current FlowStatus from state; no new version in ListFlowVersions.
Actual (emulator): HTTP 201 with a CreateFlowVersionResponse carrying a numbered version; no error code and no error message.

Smithy: CreateFlowVersion http {method: POST, uri: /flows/{flowIdentifier}/versions, code: 201}; errors include ConflictException (client, httpError 409); output.version = NumericalVersion @pattern ^[0-9]{1,5}$; FlowStatus = [Failed, Prepared, Preparing, NotPrepared].

Source: services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda/src/ops/create_flow_version.rs — ConflictException declared at line 121 but never constructed; the handler (lines 175-298) goes from flow lookup (line 234) and quota checks (lines 250-273) straight to snapshotting the version into state (lines 279-295) with no FlowStatus pre-condition.

## Steps to Reproduce
1. bedrock-agent CreateFlow with a valid minimal definition and a flow execution role ARN; note the returned flow id. GetFlow confirms status is not Prepared (NotPrepared).
2. bedrock-agent ListFlowVersions on that id and record the listing.
3. bedrock-agent CreateFlowVersion(flowIdentifier=<id>) WITHOUT calling PrepareFlow first.
   Observed: HTTP 201 with a numbered version and no error members.
   Expected: HTTP 409 with x-amzn-errortype ConflictException and message "Couldn't perform CreateFlowVersion operation. The resource that you specified is currently in status NotPrepared. Retry your request after the resource changes to a valid status."
4. bedrock-agent ListFlowVersions again: the emulator shows a leaked version; AWS's listing is unchanged.
5. Replay fixture bedrock-agent-flow-error-paths through step task#4 to reproduce the same divergence from the recording.
