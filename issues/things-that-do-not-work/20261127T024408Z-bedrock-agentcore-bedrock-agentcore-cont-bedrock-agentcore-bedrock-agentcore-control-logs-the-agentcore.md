Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore__bedrock-agentcore-control__logs.deleted-runtime-retains-runtime-log-group-and-events bedrock-agentcore__bedrock-agentcore-control__logs.runtime-invocation-leaves-undelivered-vended-group-empty
Issue-Key: bedrock-agentcore__bedrock-agentcore-control__logs.deleted-runtime-retains-runtime-log-group-and-events|_poll#10|Timeout|last = probe()
Issue-Key: bedrock-agentcore__bedrock-agentcore-control__logs.runtime-invocation-leaves-undelivered-vended-group-empty|_poll#8|Timeout|last = probe()
Issue-Commit: 34ec76825aeb60426f74f39ee1ee1124c6afb7e4

# bedrock-agentcore__bedrock-agentcore-control__logs: the AgentCore runtime invoke-and-log-delivery scenario reaches no verdict — both runtime log-group checks exhaust the runner budget

> _Note: this issue was identified against the code base at commit `34ec76825aeb60426f74f39ee1ee1124c6afb7e4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InvokeAgentRuntime

## Description
Cause 1 of 1: the end-to-end AgentCore runtime path — CreateAgentRuntime from an S3 `codeConfiguration` artifact, GetAgentRuntime settling `READY`, InvokeAgentRuntime running the deployed code, and that code's stdout reaching the runtime's CloudWatch Logs group — never completes. Both checks in this group that exercise it run out the runner budget (60 s) with no verdict; both control-plane-only AgentCore groups in the same suite pass, so the divergence is on the invoke/log-delivery path, not on the AgentCore control surface generally.

AWS: recorded behavior only (the Smithy model types the operations but states none of this sequencing). What the recordings show: CreateAgentRuntime with `agentRuntimeArtifact.codeConfiguration` (`runtime: PYTHON_3_13`, `entryPoint: ["main.py"]`, code read from an S3 prefix) is accepted, the runtime leaves `CREATING` and settles `READY` (measured at ~20-30 s); the service creates the runtime's log group `/aws/bedrock-agentcore/runtimes/<agentRuntimeId>-DEFAULT` at create time with NO `retentionInDays`, so it never expires; InvokeAgentRuntime with a `runtimeSessionId` runs the deployed HTTP agent (POST /invocations) and answers that code's body; whatever the code wrote to stdout arrives in that log group within ~7 s, in a stream whose name contains `[runtime-logs-<runtimeSessionId>]`; DeleteAgentRuntime removes the runtime (GetAgentRuntime then ResourceNotFoundException, ListAgentRuntimes no longer lists it) while the log group and its events stay readable; and the vended groups `/aws/vendedlogs/bedrock-agentcore/runtime/APPLICATION_LOGS/<id>` and `.../USAGE_LOGS/<id>` are filled ONLY by a CloudWatch Logs delivery, so with no delivery source they hold zero streams and never see the invocation's stdout. The fixed parts of that contract are the two log-group name templates, the absence of retention on the service-made group, and the `[runtime-logs-<session>]` stream-name marker; the ids, session and marker text are per-run values.

Emulator: neither check reaches any of its assertions — each ends `Check timed out after 60.0s`, so the sequence stalls somewhere between CreateAgentRuntime and the marker arriving in the runtime group. Nothing in the replay output shows a refusal, so the path does not fail fast: it hangs (a runtime that never leaves CREATING, an invocation that never returns, or stdout that never reaches the log group all present this way). Hypothesis: the deployed code is never actually run and shipped to logs, which is what both checks are gated on.

Why: a client cannot observe an AgentCore runtime doing any work. The invocation's answer and the runtime's logs are the only evidence a deployed agent ran, so an operator debugging an agent sees an empty runtime log group, and the negative guarantee the second check pins (vended APPLICATION_LOGS/USAGE_LOGS groups stay empty until a delivery is configured) is equally unverifiable — the emulator never gets far enough to be right or wrong about it.

Fix: the bedrock-agentcore InvokeAgentRuntime handler and the runtime's log-delivery path (`services/service-com-amazonaws-bedrockagentcore-amazonbedrockagentcore/src/ops/invoke_agent_runtime.rs` and the state it reads the deployed artifact through), together with the bedrock-agentcore-control CreateAgentRuntime/GetAgentRuntime state that must settle the runtime READY and create the `-DEFAULT` runtime log group; each step must either complete or refuse promptly rather than hang.

Traces: bedrock-agentcore__bedrock-agentcore-control__logs.deleted-runtime-retains-runtime-log-group-and-events, bedrock-agentcore__bedrock-agentcore-control__logs.runtime-invocation-leaves-undelivered-vended-group-empty. Related open issue that does NOT own these checks: 20261023T113357Z-bedrock-agentcore-invokeagentruntime-stub.md (filed when the operation was a scaffold stub; the handler now exists, and these two checks fail by stalling, not by a NotImplemented refusal).

## Evidence
Both failing checks in the group, verbatim from the replay report:

#### [mismatch] bedrock-agentcore__bedrock-agentcore-control__logs.deleted-runtime-retains-runtime-log-group-and-events
Error: Check timed out after 60.0s

#### [mismatch] bedrock-agentcore__bedrock-agentcore-control__logs.runtime-invocation-leaves-undelivered-vended-group-empty
Error: Check timed out after 60.0s

Per-service pass rate: bedrock-agentcore__bedrock-agentcore-control__logs 0/2 (0%), while bedrock-agentcore-control__logs 5/5 and bedrock-agentcore-control__iam__logs 2/2 pass — the two failures are the ones that invoke the runtime and read its logs.

Assertions neither check ever reached (what AWS answers):
- describe_log_groups(logGroupNamePrefix='/aws/bedrock-agentcore/runtimes/<id>-DEFAULT') returns exactly that group, with no 'retentionInDays' key.
- invoke_agent_runtime(payload={'marker': <hex>}) returns {'echo': <hex>}.
- filter_log_events on the runtime group with filterPattern '"<marker>"' yields exactly one event 'SIM2REAL-MARK <marker>' in a stream whose name contains '[runtime-logs-<runtimeSessionId>]'.
- after DeleteAgentRuntime: GetAgentRuntime -> ResourceNotFoundException and the runtime no longer listed, while the log group lookup and the marker event are unchanged.
- with no delivery source, describe_log_streams on /aws/vendedlogs/bedrock-agentcore/runtime/APPLICATION_LOGS/<id> and .../USAGE_LOGS/<id> returns [] and neither holds the marker.

## Steps to Reproduce
1. s3.CreateBucket and PutObject a zip containing main.py (an HTTP server answering GET /ping with 200 and, on POST /invocations, printing 'SIM2REAL-MARK <marker>' to stdout and echoing {'echo': marker}).
2. iam.CreateRole trusted by bedrock-agentcore.amazonaws.com with logs:* and s3:GetObject on that bucket.
3. bedrock-agentcore-control.CreateAgentRuntime with agentRuntimeArtifact.codeConfiguration {code.s3={bucket,prefix}, runtime=PYTHON_3_13, entryPoint=["main.py"]}, networkConfiguration.networkMode=PUBLIC, protocolConfiguration.serverProtocol=HTTP.
4. Poll bedrock-agentcore-control.GetAgentRuntime until status leaves CREATING — AWS settles READY.
5. logs.DescribeLogGroups with prefix /aws/bedrock-agentcore/runtimes/<agentRuntimeId>-DEFAULT — AWS already has the group, with no retentionInDays.
6. bedrock-agentcore.InvokeAgentRuntime(agentRuntimeArn, runtimeSessionId, payload={'marker': <hex>}) — AWS returns {'echo': <hex>}.
7. logs.FilterLogEvents on that group with filterPattern '"<marker>"' — AWS delivers the agent's stdout line within seconds, in a stream naming the session.
In the emulator the sequence never gets through these steps inside the runner budget: both checks end 'Check timed out after 60.0s'.
