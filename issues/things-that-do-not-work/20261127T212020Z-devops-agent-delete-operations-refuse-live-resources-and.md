Issue-Source: trace-sweep-divergence
Issue-Trace: devops-agent-delete-asset-file-path-validation
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: devops-agent-delete-asset-file-path-validation devops-agent-list-chats-page-size-and-deleted-space-refusals
Issue-Shape-Id: com.amazonaws.devopsagent#DevOpsAgent
Issue-Key: devops-agent|DeleteAssetFile|2xx->4xx|$
Issue-Key: devops-agent|DeleteAssetFile|2xx->4xx|status
Issue-Key: devops-agent|GetAsset|4xx->2xx|$.message
Issue-Key: devops-agent|GetAsset|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: devops-agent|GetAsset|4xx->2xx|status
Issue-Key: devops-agent|ListAssetFiles|4xx->2xx|$.message
Issue-Key: devops-agent|ListAssetFiles|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: devops-agent|ListAssetFiles|4xx->2xx|status
Issue-Key: devops-agent|DeleteAgentSpace|2xx->4xx|status
Issue-Commit: 9bfc3dcca28986b1e030db2b6d5b17f08a507141

# devops-agent delete operations refuse live resources and leave deleted ones readable

> _Note: this issue was identified against the code base at commit `9bfc3dcca28986b1e030db2b6d5b17f08a507141`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteAssetFile, GetAsset, ListAssetFiles, DeleteAgentSpace

## Description
Four net-new wire divergence clusters in devops-agent that share one fault: the delete paths disagree with AWS about what exists. A delete of a resource that IS present is refused 404, a delete of a space that IS present is refused 404, and the asset that AWS removes as a consequence of a delete stays readable with HTTP 200. These are status-level divergences, the most visible kind to a client. A fixer must close all four before submitting.

=== Cause 1 of 4: DeleteAssetFile refuses an existing file with 404 ResourceNotFoundException ===
AWS: Smithy: DeleteAssetFile DELETE /asset/agent-space/{agentSpaceId}/assets/{assetId}/files/{path+} -> HTTP 200 with an EMPTY body ({}), @idempotent; ResourceNotFoundException (404) is modeled for a path that does not exist. Recorded behavior: deleting the file the asset actually holds — including a file whose path is the maximum modeled length (512 characters, AssetFilePath @length 1..512) — succeeds with 200 and an empty body.
Emulator: answers 404 ResourceNotFoundException with message "Asset file not found: <the requested path>" (fixed text "Asset file not found: " plus the path taken from the request) for a path the asset holds, so the file can never be deleted.
Why: a client cannot delete an asset's file at all on this path; the delete is reported as a missing resource even though the preceding create/list calls show it present. Hypothesis: the stored key for the file and the key the delete handler looks up disagree for long or maximum-length paths (the greedy {path+} label), since the create and list of the same path succeed.
Fix: the DeleteAssetFile handler's path lookup (services/service-com-amazonaws-devopsagent-devopsagent/src/ops/delete_asset_file.rs) against the asset-file keys in src/state.rs — resolve the greedy path label the same way the create/list paths store it, and answer 200 with an empty body.
Traces: devops-agent-delete-asset-file-path-validation#task#7.

=== Cause 2 of 4: GetAsset answers 200 for an asset AWS reports as deleted ===
AWS: Smithy: GetAsset GET .../assets/{assetId} -> 200, errors include ResourceNotFoundException (404). Recorded behavior: once the asset's last file has been deleted the asset itself is gone, and GetAsset is refused 404 ResourceNotFoundException with message "Asset not found: <assetId>" — fixed text "Asset not found: " plus the assetId from the request — and the response carries the x-amzn-errortype header naming the error.
Emulator: answers HTTP 200 with the asset body: no error, no message, no x-amzn-errortype header. The asset was never removed.
Why: a client reads a deleted asset back as live, so deletion is unobservable and any read-after-delete check passes when it must fail.
Fix: the asset lifecycle in src/state.rs — removing an asset's last file removes the asset — and the GetAsset handler's not-found path (src/ops/get_asset.rs) so a missing asset is refused 404 ResourceNotFoundException with that message and error-type header.
Traces: devops-agent-delete-asset-file-path-validation#task#8.

=== Cause 3 of 4: ListAssetFiles answers 200 for an asset AWS reports as deleted ===
AWS: Smithy: ListAssetFiles GET .../assets/{assetId}/files -> 200, errors include ResourceNotFoundException (404). Recorded behavior: for an asset that no longer exists the call is refused 404 ResourceNotFoundException with the entirely fixed message "Asset does not exist" (it names neither the asset id nor a version) and an x-amzn-errortype header.
Emulator: answers HTTP 200 with an items list, no message and no error-type header.
Why: a client listing a deleted asset's files gets a success and a (possibly stale) listing instead of the modeled refusal.
Fix: the ListAssetFiles handler's asset lookup (src/ops/list_asset_files.rs) plus the same asset lifecycle fix as cause 2 — refuse a missing asset 404 ResourceNotFoundException "Asset does not exist".
Traces: devops-agent-delete-asset-file-path-validation#task#9.

=== Cause 4 of 4: DeleteAgentSpace refuses an existing space with 404 instead of 204 ===
AWS: Smithy: DeleteAgentSpace DELETE /v1/agentspaces/{agentSpaceId} -> HTTP 204 with no body, @idempotent; ResourceNotFoundException (404) is modeled only for a space that does not exist. Recorded behavior: deleting a space that the same fixture created and used returns 204.
Emulator: answers 404 for that live space, so the space cannot be deleted and the teardown of any flow that creates a space fails.
Why: a client cannot delete an agent space it just created; resources leak and every clean-up path reports the space as missing while other calls still serve it. Hypothesis: the handler resolves the {agentSpaceId} label against a different key than the one CreateAgentSpace stored (see the separate id-form defect filed for CreateAgentSpace), or an already-deleted/archived marker makes the space invisible to delete only.
Fix: the DeleteAgentSpace handler's space lookup (src/ops/delete_agent_space.rs) against the agent-space map in src/state.rs — resolve the space the same way the read paths do and answer 204 with no body.
Traces: devops-agent-list-chats-page-size-and-deleted-space-refusals#teardown#9.

## Evidence
Replay of simparity-wire-aws-v1. These are status divergences.

Cause 1 — devops-agent-delete-asset-file-path-validation#task#7, DeleteAssetFile: expected 200 with body {}; actual 404 with {'__type': 'ResourceNotFoundException', 'message': 'Asset file not found: qqqq…'} (the message carries the 512-character path from the request).

Cause 2 — devops-agent-delete-asset-file-path-validation#task#8, GetAsset: expected 404 with $.message 'Asset not found: <assetId>' and an x-amzn-errortype header; actual 200 — $.message missing, x-amzn-errortype missing.

Cause 3 — devops-agent-delete-asset-file-path-validation#task#9, ListAssetFiles: expected 404 with $.message 'Asset does not exist' and an x-amzn-errortype header; actual 200 — $.message missing, x-amzn-errortype missing.

Cause 4 — devops-agent-list-chats-page-size-and-deleted-space-refusals#teardown#9, DeleteAgentSpace: expected status 204; actual status 404.

The asset id and the 512-character path in the recorded messages are this run's values; the requirements are the statuses, the error codes and the message templates.

## Steps to Reproduce
Boot the emulator from this checkout and replay the named fixture steps.
1. Create an asset holding one file whose path is 512 characters long, then DELETE /asset/agent-space/{agentSpaceId}/assets/{assetId}/files/<that path> — expect 200 and an empty body; the emulator answers 404 'Asset file not found: <path>'.
2. After that delete, GET /asset/agent-space/{agentSpaceId}/assets/{assetId} — expect 404 ResourceNotFoundException 'Asset not found: <assetId>'; the emulator answers 200.
3. And GET .../assets/{assetId}/files — expect 404 ResourceNotFoundException 'Asset does not exist'; the emulator answers 200.
4. POST /v1/agentspaces to create a space, use it, then DELETE /v1/agentspaces/{agentSpaceId} — expect 204; the emulator answers 404.
