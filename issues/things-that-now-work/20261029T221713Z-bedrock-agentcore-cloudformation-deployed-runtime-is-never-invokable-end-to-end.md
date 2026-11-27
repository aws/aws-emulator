Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore.deploy-agent-runtime-invokable-via-cfn
Issue-Shape-Id: com.amazonaws.bedrockagentcore#AmazonBedrockAgentCore
Issue-Key: bedrock-agentcore.deploy-agent-runtime-invokable-via-cfn|_poll_until#22|Timeout|time.sleep(interval_s)
Issue-Commit: baf30d4927ba43bb30691a1e04213021d426cca6

# A CloudFormation-deployed AgentCore runtime is never invokable end to end: the deployed code is never what answers an invocation

> _Note: this report carries the end-to-end requirement of the behavioral check
> `bedrock-agentcore.deploy-agent-runtime-invokable-via-cfn`, which was cause 2 of the bundled
> report `20261029T221712Z-bedrock-agentcore-a-b-test-data-plane-and-cfn-deployed.md`. That
> report's cause 1 (the A/B-test data-plane family) has been implemented, so it moved to
> `issues/things-that-now-work/`; this cause is NOT fixed and is filed here so it stays in the
> open record under its own check id. The two narrower reports named below each own one half of
> the capability this one needs; neither asserts this end-to-end requirement._

## Operation
InvokeAgentRuntime

## Description
A runtime deployed from a code artifact through CloudFormation must be reachable: the stack's
exported `AgentRuntimeArn` must resolve to a runtime this emulator holds, and an
`InvokeAgentRuntime` against it must deliver the request payload to that runtime's deployed entry
point and return what the deployed code answered. Today neither half happens, so a client gets a
green stack, an ARN, and an unmodeled 501 on every invocation; the behavioral check polls the
invoke, swallows each refusal and is killed when its wall-clock budget expires, so the failure
surfaces as a timeout with no error text.

AWS: Smithy models `InvokeAgentRuntime` as `POST /runtimes/{agentRuntimeArn}/invocations` -> 200
with a required `Content-Type` response header and a streaming `response` `@httpPayload`; its
modeled errors are `ValidationException` (400), `ResourceNotFoundException` (404),
`RetryableConflictException` (409), `RuntimeClientError` (424), `AccessDeniedException` (403),
`ServiceQuotaExceededException` (402), `ThrottlingException` (429) and `InternalServerException`
(500). `AgentRuntimeArtifact`'s `codeConfiguration` is `code` (an S3 location) plus `runtime`
(`AgentManagedRuntimeType`, e.g. `PYTHON_3_13`) plus `entryPoint` (source: smithy_query
query="shape" shape="AgentRuntimeArtifact,CodeConfiguration"): AWS builds a container from that
artifact and runs it, and the invocation is delivered to that container's `POST /invocations`.
Recorded behavior: a runtime deployed from an S3 code artifact
(`AWS::BedrockAgentCore::Runtime`, `EntryPoint ['main.py']`, `Runtime PYTHON_3_13`,
`ProtocolConfiguration HTTP`, `NetworkMode PUBLIC`) reaches `CREATE_COMPLETE`, and the invocation
returns the body the deployed code produced — it echoes a per-run marker taken from the request
payload, so the requirement is that the payload reaches the deployed code and that code's own
answer is returned, not that any particular body is produced.

Emulator: two gaps, each already filed on its own and neither asserting this end-to-end
requirement.

1. `services/service-com-amazonaws-cloudformation-cloudformation/src/state/template/return_values.rs`
   derives an `AgentRuntimeId` and `AgentRuntimeArn` for the stack's outputs and says so in its own
   comment — the members that describe a resource's running state are not carried "because nothing
   here provisioned a resource to be in a state". So the exported ARN resolves to no runtime the
   AgentCore control plane holds. Filed as
   `20261029T212239Z-bedrock-agentcore-control-cloudformation-does-not-provision-agentcore-resources-aws-bedrockagentcore.md`.
2. `services/service-com-amazonaws-bedrockagentcore-amazonbedrockagentcore/src/ops/invoke_agent_runtime.rs`
   still carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and answers HTTP 501
   `NotImplemented`, which is not a modeled error for this operation. Filed as
   `20261023T113357Z-bedrock-agentcore-invokeagentruntime-stub.md`, grounded in a different check
   id.

Why: infrastructure-as-code is the normal way an AgentCore runtime is created, and invocation is
the only thing a runtime is for. Fixing either half alone leaves the check failing: a provisioned
runtime nothing can invoke still answers 501, and an implemented invoke still finds nothing behind
the stack's ARN.

## What a fix has to do
* Make `AWS::BedrockAgentCore::Runtime` provision a runtime the AgentCore control plane itself
  returns from its own read, so the exported `AgentRuntimeArn` resolves to a real resource rather
  than to an identifier CloudFormation derived.
* Make `InvokeAgentRuntime` resolve that ARN (404 `ResourceNotFoundException` when absent), deliver
  the request payload to the runtime's deployed entry point, and return what the deployed code
  answered through the streaming `response` payload with `Content-Type` set.
* The second bullet needs the deployed code to RUN. For a `codeConfiguration` runtime that means a
  contained, bounded in-process interpreter for the artifact's language, and the AgentCore runtime
  contract the entry point serves `POST /invocations` through. Nothing may be synthesized: a body
  this emulator composed itself, or an echo of the request, is not the deployed code's answer, and
  one refusal swapped for another (the 501 for a modeled error) is not a fix either.

## Evidence
Replay report (simparity-behavioral-aws-v1), verdict 'mismatch' with detail
`Error: Check timed out after 60.0s`:
- `bedrock-agentcore.deploy-agent-runtime-invokable-via-cfn` — `CreateStack` with one
  `AWS::BedrockAgentCore::Runtime` (Code S3 bucket/prefix, `EntryPoint ['main.py']`, `Runtime
  PYTHON_3_13`, `ProtocolConfiguration HTTP`, `NetworkMode PUBLIC`) and output
  `RuntimeArn = Fn::GetAtt Runtime.AgentRuntimeArn`, then polls `InvokeAgentRuntime` (payload
  `{'prompt': <per-run marker>}`) for up to 280s, swallowing each failure.
  Expected (AWS): 200 and the body the deployed code produced, i.e. the marker the request sent
  reaching that code and its answer coming back.
  Actual (emulator): HTTP 501 `NotImplemented`, `operation InvokeAgentRuntime is scaffolded but not
  implemented in this emulator`, on every attempt, and no runtime behind the stack's exported ARN.
Hosts, bucket names, role names, stack names, generated ids and the per-run marker differ on every
run and are not part of the requirement.

## Steps to Reproduce
1. Boot the emulator.
2. Upload the agent code artifact to S3, then `CreateStack` with a template containing one
   `AWS::BedrockAgentCore::Runtime` whose `Code` points at that artifact and whose output is
   `Fn::GetAtt Runtime.AgentRuntimeArn`.
3. Call `GetAgentRuntime` on the exported ARN: the AgentCore control plane holds no such runtime.
4. Call `InvokeAgentRuntime` on that ARN with payload `{"prompt": "<marker>"}` and
   `contentType`/`accept` `application/json`. Observe HTTP 501 `NotImplemented` instead of 200
   carrying what the deployed code answered.
