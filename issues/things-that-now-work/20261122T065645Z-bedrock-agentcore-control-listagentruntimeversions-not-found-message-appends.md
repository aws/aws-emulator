Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agentcore-control-agentcore-traj-runtime-lookup-from-log-group-name
Issue-Step: task#8
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agentcore-control-agentcore-traj-runtime-lookup-from-log-group-name
Issue-Shape-Id: com.amazonaws.bedrockagentcorecontrol#AmazonBedrockAgentCoreControl
Issue-Key: bedrock-agentcore-control|ListAgentRuntimeVersions|4xx->4xx|$.message
Issue-Commit: b77e21fea94e07e63ec5ef73ffc6c20098a6e938

# bedrock-agentcore-control ListAgentRuntimeVersions not-found message appends a version clause the operation has no version input for

> _Note: this issue was identified against the code base at commit `b77e21fea94e07e63ec5ef73ffc6c20098a6e938`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListAgentRuntimeVersions

## Description
Cause 1 of 1: ListAgentRuntimeVersions on an agent runtime id that names no runtime returns ResourceNotFoundException with the right status and error code but the wrong message — the emulator emits the version-qualified not-found template used by the version-aware lookups and fills the version slot with the literal 'null', even though ListAgentRuntimeVersions has no version input at all.

AWS: ListAgentRuntimeVersions (Smithy, com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol.json: POST /runtimes/{agentRuntimeId}/versions/, success 200, readonly, paginated by nextToken/maxResults; input members are ONLY agentRuntimeId (httpLabel, required, pattern ^[a-zA-Z][a-zA-Z0-9_]{0,99}-[a-zA-Z0-9]{10}$), maxResults (httpQuery, range 1..100) and nextToken (httpQuery, length 1..2048); modeled errors AccessDeniedException 403, InternalServerException 500, ResourceNotFoundException 404, ThrottlingException 429, ValidationException 400). The operation enumerates ALL versions of one runtime, so no version is ever selected by the caller — unlike GetAgentRuntime, whose input does carry agentRuntimeVersion (httpQuery 'version', pattern ^([1-9][0-9]{0,4})$). When the runtime does not exist AWS answers HTTP 404 with x-amzn-errortype ResourceNotFoundException and an agent-scoped, version-FREE message. The model documents no message text, so the exact wording is recorded behavior only; the recorded response shows it as "Agent with agentId: <agentRuntimeId>, accountId: <accountId> not found!". Template breakdown: the fixed text is "Agent with agentId: ", ", accountId: " and " not found!"; the variable parts are the agentRuntimeId path label taken verbatim from the request URI and the 12-digit accountId of the calling identity resolved from the emulator's own state. The sentence ENDS after the account id — there is no version clause, because the request named no version. The values in this one recording ('sim2real_probe_agmcp_ed5cafbc-kR7vPq2wXn', '000000000000') fill the template; they are not part of it.

Emulator: status 404 and error code ResourceNotFoundException are correct, but the message is "Agent with agentId: <agentRuntimeId>, accountId: <accountId>, and version: null not found!" — in this replay 'Agent with agentId: sim2real_probe_agmcp_ed5cafbc-kR7vPq2wXn, accountId: 000000000000, and version: null not found!'. The emulator uses one shared not-found formatter for the whole /runtimes/{agentRuntimeId} family: it unconditionally interpolates the optional version member, and on an operation that has no such member the Option renders as the literal string 'null', producing the extra fixed clause ", and version: " plus a bogus value.

Why: a client relying on AWS's behavior reads a message asserting that a specific version of the runtime was looked up and missing, when no version was ever requested and the real condition is that the runtime itself does not exist. Since every one of these conditions shares HTTP 404 and ResourceNotFoundException, message text is the only discriminator, so error-classification code, log scrapers and assertions that match AWS's wording ("... accountId: <id> not found!") fail against the emulator, and anything parsing the version out of the message gets the string 'null'. It also mis-reports the fault class: 'version: null' reads as a malformed-version bug in the caller rather than a missing runtime.

Fix: the bedrock-agentcore-control ListAgentRuntimeVersions handler's not-found path (and the shared runtime not-found message formatter it calls) — the version clause must be emitted only when the operation's input actually carries a version and one was supplied, and omitted entirely for ListAgentRuntimeVersions, which has no version member. The lookup should resolve agentRuntimeId alone and raise ResourceNotFoundException with the version-free agent-scoped message, interpolating the request's agentRuntimeId and the caller's account id.

Traces: fixture bedrock-agentcore-control-agentcore-traj-runtime-lookup-from-log-group-name, step task#8 (cluster of 1).

## Evidence
Replay of fixture bedrock-agentcore-control-agentcore-traj-runtime-lookup-from-log-group-name, step task#8, operation ListAgentRuntimeVersions — verdict mismatch, affected_count 1, owned_by [] and stub_path "" (not already filed, not blocked by a stub).

Status: expected 404, actual 404 (matches).
Error type: ResourceNotFoundException (matches).
$.message: value_mismatch
  expected: "Agent with agentId: sim2real_probe_agmcp_ed5cafbc-kR7vPq2wXn, accountId: 000000000000 not found!"
  actual:   "Agent with agentId: sim2real_probe_agmcp_ed5cafbc-kR7vPq2wXn, accountId: 000000000000, and version: null not found!"

divergence_key: service=bedrock-agentcore-control|ListAgentRuntimeVersions|status=404->404|keys=$.message

The sole difference is the emulator's extra clause ", and version: null" inserted before " not found!". Fixed text of the AWS message: "Agent with agentId: " + <agentRuntimeId from request path> + ", accountId: " + <caller account id from state> + " not found!". No version, no request id, no timestamp — nothing per-run beyond those two interpolations, and both of them agree between expected and actual here, so the divergence is purely the spurious version clause.

Smithy evidence: ListAgentRuntimeVersions input shape ListAgentRuntimeVersionsRequest contains agentRuntimeId, maxResults, nextToken ONLY — no agentRuntimeVersion member exists on this operation, so there is no value the 'version:' slot could legitimately report. Compare GetAgentRuntime, whose input DOES declare agentRuntimeVersion (httpQuery 'version'); that is the operation whose message template the emulator appears to be reusing. ResourceNotFoundException is modeled on ListAgentRuntimeVersions with httpError 404.

Related but NOT covering this: open issue 20261119T134619Z-bedrock-agentcore-control-deleteagentruntimeendpoint-not-found-message-names.md is the same family of defect (a runtime not-found message naming the wrong thing) but is scoped to DeleteAgentRuntimeEndpoint's endpoint-vs-parent message and carries Issue-Key bedrock-agentcore-control|DeleteAgentRuntimeEndpoint|4xx->4xx|$.message; it does not touch ListAgentRuntimeVersions or the spurious version clause. No open issue mentions ListAgentRuntimeVersions at all.

## Steps to Reproduce
1. Boot the emulator from this checkout (bedrock-agentcore-control is serving; get_bootstrap_status reports no pending services).
2. Replay fixture bedrock-agentcore-control-agentcore-traj-runtime-lookup-from-log-group-name and stop at step task#8, or issue the call directly:
     POST /runtimes/sim2real_probe_agmcp_ed5cafbc-kR7vPq2wXn/versions/
   against the bedrock-agentcore-control endpoint, with no runtime of that id ever created (any id matching ^[a-zA-Z][a-zA-Z0-9_]{0,99}-[a-zA-Z0-9]{10}$ that does not exist reproduces it).
3. Observe HTTP 404 with x-amzn-errortype: ResourceNotFoundException and body message "Agent with agentId: <id>, accountId: <account>, and version: null not found!".
4. AWS answers the same 404/ResourceNotFoundException but with "Agent with agentId: <id>, accountId: <account> not found!" — the message ends after the account id, with no version clause, because ListAgentRuntimeVersions accepts no version.
5. Contrast with GET /runtimes/<missing-id>/ (GetAgentRuntime), which DOES accept ?version=<n>: only an operation with a version input may mention a version in its not-found message, and never as the literal 'null'.
