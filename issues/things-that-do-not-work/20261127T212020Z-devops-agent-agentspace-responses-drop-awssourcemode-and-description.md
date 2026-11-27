Issue-Source: trace-sweep-divergence
Issue-Trace: devops-agent-agent-space-tags-and-update
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: devops-agent-agent-space-tags-and-update devops-agent-get-asset-content-version-bounds devops-agent-get-asset-file-input-bounds devops-agent-list-asset-files-paging-bounds devops-agent-list-assets-filter-and-paging-validation devops-agent-list-pending-messages-unknown-execution devops-agent-trigger-and-goal-unknown-id-paths devops-agent-list-chats-max-results-bounds devops-agent-list-journal-records-input-validation
Issue-Shape-Id: com.amazonaws.devopsagent#DevOpsAgent
Issue-Key: devops-agent|CreateAgentSpace|2xx->2xx|$.agentSpace.awsSourceMode
Issue-Key: devops-agent|CreateAgentSpace|2xx->2xx|$.agentSpace.createdAt
Issue-Key: devops-agent|CreateAgentSpace|2xx->2xx|$.agentSpace.updatedAt
Issue-Key: devops-agent|GetAgentSpace|2xx->2xx|$.agentSpace.awsSourceMode
Issue-Key: devops-agent|GetAgentSpace|2xx->2xx|$.agentSpace.createdAt
Issue-Key: devops-agent|GetAgentSpace|2xx->2xx|$.agentSpace.description
Issue-Key: devops-agent|GetAgentSpace|2xx->2xx|$.agentSpace.updatedAt
Issue-Key: devops-agent|CreateAgentSpace|2xx->2xx|$.agentSpace.agentSpaceId
Issue-Commit: 9bfc3dcca28986b1e030db2b6d5b17f08a507141

# devops-agent AgentSpace responses drop awsSourceMode and description and serialize timestamps as epoch numbers

> _Note: this issue was identified against the code base at commit `9bfc3dcca28986b1e030db2b6d5b17f08a507141`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAgentSpace, GetAgentSpace

## Description
Three net-new wire divergence clusters in devops-agent that share one fault: the AgentSpace structure the emulator projects into CreateAgentSpace and GetAgentSpace responses is incomplete and mis-serialized. All three carry HTTP 200/201 as AWS does — the divergence is the body. A fixer must close all three before submitting.

=== Cause 1 of 3: CreateAgentSpace omits AgentSpace$awsSourceMode and serializes createdAt/updatedAt as epoch numbers ===
AWS: Smithy: CreateAgentSpace POST /v1/agentspaces -> 201, output agentSpace -> AgentSpace (required). AgentSpace$awsSourceMode -> AwsSourceMode enum with values ["standalone", "organization"], documented as "The Amazon Web Services source mode for the AgentSpace, indicating whether its Amazon Web Services source configuration targets an individual Amazon Web Services account or an Amazon Web Services Organization". AgentSpace$createdAt and AgentSpace$updatedAt are both `Timestamp` with @required AND @timestampFormat("date-time"), i.e. they MUST be serialized as RFC-3339/ISO-8601 date-time STRINGS. Recorded behavior: a space created without an explicit awsSourceMode comes back with awsSourceMode = "standalone" (the single-account default), and both timestamps present as date-time strings.
Emulator: the response omits `awsSourceMode` entirely (the corpus reports it missing where "standalone" was recorded), and emits createdAt/updatedAt as bare epoch-seconds floats, which fails the corpus's shape assertion for those members.
Why: a client that switches on awsSourceMode to decide whether a space is account- or organization-scoped reads null and cannot tell; and any SDK or client that parses createdAt/updatedAt as the modeled date-time string either fails to deserialize or mis-reads a float as a date, so the create response of every agent space is unusable for both.
Fix: the AgentSpace projection used by the CreateAgentSpace handler (services/service-com-amazonaws-devopsagent-devopsagent/src/ops/create_agent_space.rs) and the agent-space record in src/state.rs — default awsSourceMode to "standalone" when the request does not set it, carry it on the stored space, and serialize AgentSpace timestamps with the modeled date-time format.
Traces: devops-agent-agent-space-tags-and-update#setup#1 (cluster also covers setup#1 of devops-agent-get-asset-content-version-bounds, devops-agent-get-asset-file-input-bounds, devops-agent-list-asset-files-paging-bounds, devops-agent-list-assets-filter-and-paging-validation, devops-agent-list-pending-messages-unknown-execution and devops-agent-trigger-and-goal-unknown-id-paths).

=== Cause 2 of 3: GetAgentSpace does not report a description set by UpdateAgentSpace, and repeats the awsSourceMode/timestamp defects ===
AWS: Smithy: GetAgentSpace GET /v1/agentspaces/{agentSpaceId} -> 200, output agentSpace -> AgentSpace (required). AgentSpace$description -> Description, length 1..1000, @sensitive, documented "The description of the AgentSpace"; UpdateAgentSpace is the modeled way to set it. Recorded behavior: after the space's description is updated, GetAgentSpace reports that description, reports awsSourceMode = "standalone", and reports createdAt/updatedAt as date-time strings (and updatedAt advances past createdAt once the space is modified).
Emulator: GetAgentSpace answers 200 but `description` is absent after the update, `awsSourceMode` is absent, and the timestamps are epoch floats — and createdAt and updatedAt are byte-identical after a mutation, so the stored space's updatedAt is never advanced.
Why: a client that writes a description and reads it back sees nothing stored, so the update appears to have been silently dropped; and a client that detects change by comparing updatedAt never sees the space change.
Fix: the agent-space record and its update path in src/state.rs plus the GetAgentSpace projection (src/ops/get_agent_space.rs) — persist description on update, stamp updatedAt on every mutation, and project awsSourceMode and the date-time-formatted timestamps.
Traces: devops-agent-agent-space-tags-and-update#task#7.

=== Cause 3 of 3: CreateAgentSpace mints an agentSpaceId of the wrong form (alongside the same awsSourceMode/timestamp defects) ===
AWS: Smithy: AgentSpace$agentSpaceId -> AgentSpaceId, @required, @pattern ^[a-zA-Z0-9-]{1,64}$ — the modeled pattern is permissive. Recorded behavior only: AWS mints the identifier as a 36-character lowercase UUID (8-4-4-4-12 lowercase hex groups joined by '-'), and the corpus's shaped assertion on agentSpaceId accepts only that form.
Emulator: returns a short sequential identifier of the form 'as-' followed by zero-padded digits, which satisfies the model pattern but not the recorded id form, so the shape assertion on the created space's id fails.
Why: a client that shape-checks, parses or keys its own records on the AgentSpace id (a 36-char UUID) rejects or mis-stores every space the emulator creates. The requirement is the id FORM, not any particular recorded id value.
Fix: the agent-space id generator in src/state.rs used by src/ops/create_agent_space.rs — mint a fresh lowercase UUID per space. NOTE: the open behavioral issue 20261127T091917Z-devops-agent-9-net-new-causes-non-uuid.md (cause 1) describes the same id-form defect from the behavioral suite; the replay does not report this wire cluster as owned by it, so it is filed here with its wire replay key and the two should be closed together.
Traces: devops-agent-list-chats-max-results-bounds#setup#1 (cluster also covers devops-agent-list-journal-records-input-validation#setup#1).

## Evidence
Replay of simparity-wire-aws-v1; statuses match AWS (201 for CreateAgentSpace, 200 for GetAgentSpace) in all three causes.

Cause 1 — devops-agent-agent-space-tags-and-update#setup#1, CreateAgentSpace, 201 -> 201:
  $.agentSpace.awsSourceMode: missing (expected 'standalone', got None)
  $.agentSpace.createdAt: shape mismatch (expected the recorded date-time form, got 1795813957.5031085)
  $.agentSpace.updatedAt: shape mismatch (expected the recorded date-time form, got 1795813957.5031085)
  7 further fixture steps in the same cluster.

Cause 2 — devops-agent-agent-space-tags-and-update#task#7, GetAgentSpace, 200 -> 200:
  $.agentSpace.description: missing (expected the description written by the preceding update, got None)
  $.agentSpace.awsSourceMode: missing (expected 'standalone', got None)
  $.agentSpace.createdAt / $.agentSpace.updatedAt: shape mismatch (both got 1795813957.5031085 — the same value as at create time, so updatedAt did not advance over the update).

Cause 3 — devops-agent-list-chats-max-results-bounds#setup#1, CreateAgentSpace, 201 -> 201:
  $.agentSpace.agentSpaceId: shape mismatch (expected the recorded 36-char lowercase-UUID form, got 'as-0000000000000001')
  $.agentSpace.awsSourceMode: missing (expected 'standalone', got None)
  $.agentSpace.createdAt / updatedAt: shape mismatch (got 1795813955.2954056).

The epoch numbers and 'as-0000000000000001' are this run's values; the requirements are the serialization form (modeled @timestampFormat("date-time") strings), the presence of awsSourceMode with its default, and the UUID id form.

## Steps to Reproduce
Boot the emulator from this checkout and replay the named fixture steps.
1. POST /v1/agentspaces with a name and no awsSourceMode — inspect the 201 body: `agentSpace.awsSourceMode` should be "standalone", `agentSpace.createdAt`/`updatedAt` should be date-time strings, `agentSpace.agentSpaceId` should be a 36-character lowercase UUID.
2. Update that space's description, then GET /v1/agentspaces/{agentSpaceId} — `agentSpace.description` should carry the new description and `updatedAt` should be later than `createdAt`.
