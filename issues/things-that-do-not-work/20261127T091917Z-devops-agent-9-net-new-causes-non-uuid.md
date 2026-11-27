Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: devops-agent.asset-version-history-follows-content-edits-and-empties-on-delete devops-agent.create-asset-file-on-occupied-skill-path-rejected devops-agent.list-triggers-denied-once-agent-space-is-deleted devops-agent.new-agent-space-seeds-weekly-oncall-goal-that-update-goal-can-disable devops-agent.tag-resource-rejects-aws-reserved-key-prefix-on-agent-space devops-agent.create-asset-file-rejects-text-over-length-limit devops-agent.delete-asset-file-rejected-for-unknown-asset-in-live-space devops-agent.delete-asset-file-unknown-path-in-skill-rejected-files-kept devops-agent.list-asset-files-lists-file-added-to-skill devops-agent.list-asset-files-pages-return-every-skill-file-exactly-once devops-agent.list-asset-files-rejects-a-next-token-it-never-issued devops-agent.artifact-file-moved-by-update-stays-listed-at-its-old-version devops-agent.get-asset-content-rejects-version-past-current-rejected devops-agent.list-asset-files-rejects-asset-version-never-written devops-agent.list-asset-versions-rejects-unissued-next-token devops-agent.update-asset-content-replaces-artifact-file devops-agent.update-asset-file-reused-client-token-with-new-content-rejected
Issue-Shape-Id: com.amazonaws.devopsagent#DevOpsAgent
Issue-Key: devops-agent.asset-version-history-follows-content-edits-and-empties-on-delete|run#3|AssertionError|assert SPACE_ID.fullmatch(space_id), f"CreateAgentSpace returned an agentSpaceId of length {len(space_id)} that is not a UUID"
Issue-Key: devops-agent.create-asset-file-on-occupied-skill-path-rejected|run#3|AssertionError|assert SPACE_ID.fullmatch(space_id), "CreateAgentSpace returned an agentSpaceId that is not a lowercase UUID"
Issue-Key: devops-agent.list-triggers-denied-once-agent-space-is-deleted|run#3|AssertionError|assert SPACE_ID.fullmatch(space_id), "CreateAgentSpace returned an agentSpaceId that is not a lowercase UUID"
Issue-Key: devops-agent.new-agent-space-seeds-weekly-oncall-goal-that-update-goal-can-disable|run#3|AssertionError|assert SPACE_ID.fullmatch(space_id), "CreateAgentSpace returned an agentSpaceId that is not a lowercase UUID"
Issue-Key: devops-agent.tag-resource-rejects-aws-reserved-key-prefix-on-agent-space|run#3|AssertionError|assert SPACE_ID.fullmatch(space_id), "CreateAgentSpace returned an agentSpaceId that is not a lowercase UUID"
Issue-Key: devops-agent|CreateAssetFile|ValidationException
Issue-Key: devops-agent|CreateAsset|ValidationException
Issue-Key: devops-agent|UpdateAssetFile|ResourceNotFoundException
Issue-Key: devops-agent.get-asset-content-rejects-version-past-current-rejected|run#19|AssertionError|assert (code, _wording(message)) == ("ResourceNotFoundException", "Asset not found: <asset>") and (
Issue-Key: devops-agent.list-asset-files-rejects-asset-version-never-written|run#9|AssertionError|assert (code, message) == ("ResourceNotFoundException", "Asset does not exist"), (
Issue-Key: devops-agent.list-asset-versions-rejects-unissued-next-token|run#14|AssertionError|assert (code, message) == ("ValidationException", "The provided NextToken is invalid or corrupted."), (
Issue-Key: devops-agent.update-asset-content-replaces-artifact-file|run#11|AssertionError|assert before == {"title": "service owners", "status": "ACTIVE"}, (
Issue-Key: devops-agent.update-asset-file-reused-client-token-with-new-content-rejected|_refusal#14|AssertionError|raise AssertionError(f"expected a refusal, the call succeeded: {answer!r}")
Issue-Commit: 4621a6ee7f1fe7bbce8e80b2d15ffa4da92b4e77

# devops-agent: 9 net-new causes — non-UUID agentSpaceId, skill assets refuse a second file, zip skill CreateAsset refused, UpdateAssetFile cannot write a new path, wrong not-found/pagination messages, stored artifact metadata, and clientToken conflict not detected

> _Note: this issue was identified against the code base at commit `4621a6ee7f1fe7bbce8e80b2d15ffa4da92b4e77`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAgentSpace, CreateAssetFile, CreateAsset, UpdateAssetFile, GetAssetContent, ListAssetFiles, ListAssetVersions, GetAsset

## Description
Nine distinct, independent defects the behavioral suite found in the devops-agent emulator. All nine belong to this one service; each is numbered below with its own failing check ids. A fixer must close all nine before submitting.

=== Cause 1 of 9: CreateAgentSpace returns an agentSpaceId that is not a lowercase UUID ===
AWS: Smithy: CreateAgentSpaceOutput.agentSpace -> AgentSpace$agentSpaceId, target AgentSpaceId, @pattern ^[a-zA-Z0-9-]{1,64}$, required. The model's pattern is permissive; recorded behavior only: AWS mints the AgentSpace identifier as a 36-character lowercase UUID (8-4-4-4-12 lowercase hex groups separated by '-'), and every id the recordings carry has that shape. Later calls in these checks (and clients that store the id) rely on that shape.
Emulator: CreateAgentSpace answers 201 with an agentSpaceId of a different, shorter form — the replay reports 'CreateAgentSpace returned an agentSpaceId of length 19 that is not a UUID'. The value matches the model pattern but is not a lowercase UUID, so every check that shape-checks the new space's id fails on the very first call.
Why: a client that persists, parses, or shape-validates the AgentSpace id (or keys its own records on a UUID) rejects or mis-stores every space the emulator creates, and no asset/goal/trigger flow in the space can be exercised.
Fix: the identifier minting in the CreateAgentSpace handler (services/service-com-amazonaws-devopsagent-devopsagent/src/ops/create_agent_space.rs) / the state's agent-space id generator — mint a fresh lowercase UUID per space.
Traces: devops-agent.asset-version-history-follows-content-edits-and-empties-on-delete, devops-agent.create-asset-file-on-occupied-skill-path-rejected, devops-agent.list-triggers-denied-once-agent-space-is-deleted, devops-agent.new-agent-space-seeds-weekly-oncall-goal-that-update-goal-can-disable, devops-agent.tag-resource-rejects-aws-reserved-key-prefix-on-agent-space.

=== Cause 2 of 9: CreateAssetFile refuses the second file of a 'skill' asset ===
AWS: Smithy: CreateAssetFile POST /asset/agent-space/{agentSpaceId}/assets/{assetId}/files/{path+}, 201, output AssetFile (path, content, version). Nothing in the model bounds a skill asset to a single file (AssetContent/assetType are unconstrained in that respect). Recorded behavior: a skill asset created with SKILL.md accepts further files such as 'references/large.md' — the call returns 201 and AssetFile$version = 2 (the asset advances one version), and ListAssetFiles then lists SKILL.md beside the added path.
Emulator: the call is refused 400 ValidationException with message "Assets of type 'skill' support only one file. Use UpdateAssetFile to replace the existing file." — fixed text, with 'skill' substituted from the asset's stored type. The guard is services/service-com-amazonaws-devopsagent-devopsagent/src/ops/create_asset_file.rs:256-260 (`if asset.content_is_one_file && !asset.files.is_empty()` -> `crate::state::asset_type_holds_one_file_message`).
Why: a skill bundle is exactly a multi-file asset (an entry point plus reference files). A client building one gets a hard 400 on every file after the first, so no multi-file skill can exist in the emulator and five downstream behaviors (listing, paging, delete-file rejection, content-length validation) are unreachable.
Fix: the CreateAssetFile handler's asset-shape validation — the one-file rule must not apply to the 'skill' asset type (it belongs only to assets whose content IS a single file), and an accepted create must advance the asset version.
Traces: devops-agent.create-asset-file-rejects-text-over-length-limit, devops-agent.delete-asset-file-rejected-for-unknown-asset-in-live-space, devops-agent.delete-asset-file-unknown-path-in-skill-rejected-files-kept, devops-agent.list-asset-files-lists-file-added-to-skill, devops-agent.list-asset-files-pages-return-every-skill-file-exactly-once.

=== Cause 3 of 9: CreateAsset of a zip-bundled skill is refused for a missing metadata name ===
AWS: Smithy: CreateAsset input assetType (required), metadata (Document — the model constrains no key), content (AssetContent, required; a zip member carries the bundle). Recorded behavior: CreateAsset with assetType='skill', metadata={"agent_types":["GENERIC"]} and content={"zip":{"zipFile": <bundle>}} is ACCEPTED (201) and the created skill holds the bundle's paths — the skill's name comes from the bundle's SKILL.md front matter, so no metadata 'name' is required when the content is a zip.
Emulator: the call is refused 400 ValidationException "name is required for Skill knowledge items" (fixed text; 'Skill' is the asset type class). The request never reaches storage.
Why: the documented way to create a skill — upload the zip bundle — cannot be used at all; a client must guess an undocumented metadata key, and the paged-file behavior behind it is unreachable.
Fix: the CreateAsset handler's metadata validation (services/service-com-amazonaws-devopsagent-devopsagent/src/ops/create_asset.rs) — do not require metadata.name for a skill whose content arrives as a zip; take the name from the bundle.
Traces: devops-agent.list-asset-files-rejects-a-next-token-it-never-issued.

=== Cause 4 of 9: UpdateAssetFile cannot write a path the asset does not already hold ===
AWS: Smithy: UpdateAssetFile PATCH .../files/{path+}, 200, output AssetFile. Recorded behavior: writing an artifact's file under a NEW path replaces the artifact's single file at the next version — the call returns AssetFile{path: <new path>, version: 2}; the latest version then lists only the new path while assetVersion=1 still lists and serves the original path and its content, and GetAsset reports version 2.
Emulator: the call is refused 404 ResourceNotFoundException "Asset file not found: <path>" (fixed text 'Asset file not found: ' plus the requested path from the request) — the handler requires the path to already exist, so an artifact's file can never be moved/renamed.
Why: a client that re-publishes an artifact under a new filename gets a 404 and cannot update the asset at all; version history for the old path is never created.
Fix: the UpdateAssetFile handler (services/service-com-amazonaws-devopsagent-devopsagent/src/ops/update_asset_file.rs) — for an asset whose content is one file, a write to a new path replaces that file at the next version instead of 404ing; the prior version keeps its path and content.
Traces: devops-agent.artifact-file-moved-by-update-stays-listed-at-its-old-version.

=== Cause 5 of 9: GetAssetContent's not-found message appends ' version <n>' ===
AWS: Smithy: GetAssetContent errors include ResourceNotFoundException (404); assetVersion is an @httpQuery integer with range min 1. Recorded behavior: asking for an assetVersion one past the asset's latest is refused 404 ResourceNotFoundException with message "Asset not found: <assetId>" — the fixed text is 'Asset not found: ' and the only variable part is the assetId from the request; the message ENDS with the asset id and never names the version.
Emulator: answers 404 ResourceNotFoundException with 'Asset not found: <assetId> version <n>' (the replay shows the id followed by ' version 3'), i.e. an extra version suffix that AWS does not emit.
Why: a client that matches or displays the refusal message (or parses the trailing id out of it) misreads the emulator's message; the error text is part of the contract.
Fix: the message builder shared by the asset handlers (crate::state asset-not-found message, used by services/service-com-amazonaws-devopsagent-devopsagent/src/ops/get_asset_content.rs) — emit 'Asset not found: ' + assetId with no version suffix.
Traces: devops-agent.get-asset-content-rejects-version-past-current-rejected.

=== Cause 6 of 9: ListAssetFiles refuses an unwritten assetVersion with the wrong message ===
AWS: Smithy: ListAssetFiles errors include ResourceNotFoundException (404); assetVersion @httpQuery, range min 1. Recorded behavior: listing an assetVersion that has never been written (version 2 of a one-version asset) is refused 404 ResourceNotFoundException with the message "Asset does not exist" — entirely fixed text, naming neither the asset id nor the version.
Emulator: answers 404 ResourceNotFoundException 'Asset not found: <assetId> version 2' — a different message (and a different wording family) for the same refusal.
Why: the same refusal reads differently from AWS, so a client keying on the message (or logging it for operators) diverges; and it leaks an id AWS does not include.
Fix: the ListAssetFiles handler's version lookup (services/service-com-amazonaws-devopsagent-devopsagent/src/ops/list_asset_files.rs) — refuse an unwritten version with the fixed 'Asset does not exist' message.
Traces: devops-agent.list-asset-files-rejects-asset-version-never-written.

=== Cause 7 of 9: ListAssetVersions rejects an unissued nextToken with the wrong message ===
AWS: Smithy: ListAssetVersions is @paginated (inputToken/outputToken nextToken), nextToken length 1..2048, errors include ValidationException (400). Recorded behavior: a nextToken this service never issued is refused 400 ValidationException with the message "The provided NextToken is invalid or corrupted." — fixed text, no variable part.
Emulator: the status and code are right (400 ValidationException) but the message is 'nextToken is not a pagination token issued by this service'.
Why: the error message is a requirement; a client or operator matching AWS's NextToken wording does not recognize the emulator's refusal.
Fix: the pagination-token validation used by the ListAssetVersions handler (services/service-com-amazonaws-devopsagent-devopsagent/src/ops/list_asset_versions.rs) — emit AWS's fixed NextToken message.
Traces: devops-agent.list-asset-versions-rejects-unissued-next-token.

=== Cause 8 of 9: GetAsset returns a submitted description in an artifact's stored metadata ===
AWS: Smithy: CreateAsset.metadata and Asset$metadata are Document shapes, so the model names no keys — recorded behavior only: an artifact created with metadata {"title": <title>, "description": <text>} stores only the title plus a service-set "status": "ACTIVE"; the submitted description is NOT stored, so GetAsset's asset.metadata is exactly {"title": <the submitted title>, "status": "ACTIVE"} and a content-only UpdateAsset leaves that metadata unchanged.
Emulator: GetAsset returns the submitted description as well — the replay shows metadata {'status': 'ACTIVE', 'description': 'who answers for each service', 'title': 'service owners'} where AWS stores {'title': ..., 'status': 'ACTIVE'}.
Why: a client reading an artifact's metadata back sees a key AWS drops, so round-trip comparisons and metadata-diffing logic disagree with AWS.
Fix: the asset metadata normalization on create (services/service-com-amazonaws-devopsagent-devopsagent/src/ops/create_asset.rs and the asset record in src/state.rs) — keep only the metadata keys AWS stores for the asset type and add the service-set status.
Traces: devops-agent.update-asset-content-replaces-artifact-file.

=== Cause 9 of 9: UpdateAssetFile accepts a reused clientToken carrying different content ===
AWS: Smithy: UpdateAssetFile.clientToken is @idempotencyToken; ConflictException (409) is a modeled error of the operation. Recorded behavior: replaying a token with the SAME content returns the stored update without adding a version, while the same token with DIFFERENT content is refused 409 ConflictException with the message "A different request was already submitted with this clientToken." (fixed text, no variable part) and stores nothing.
Emulator: the third call — same clientToken, new content — succeeds with HTTP 200 and returns the previously stored file (version 2, the earlier content) instead of refusing; the emulator replays the cached response without comparing the request to the one the token recorded.
Why: a client that reuses a token by mistake gets a silent, misleading success (the returned content is not the content it sent and its write is lost) where AWS tells it the token conflicts.
Fix: the idempotency-token bookkeeping in the UpdateAssetFile handler (services/service-com-amazonaws-devopsagent-devopsagent/src/ops/update_asset_file.rs, the token record written via crate::state::record_client_token) — record a fingerprint of the request with the token and answer ConflictException when a replay's request differs.
Traces: devops-agent.update-asset-file-reused-client-token-with-new-content-rejected.

## Evidence
Suite: 13/58 passed; devops-agent 13/55 with 42 failures. Checks blocked by unimplemented ListAssets/ListGoals/ListAssetTypes/ListBacklogTasks/ListJournalRecords/ListTriggers/UpdateAsset/UpdateTrigger stubs are already filed and excluded here; the three cloudformation__devops-agent checks are OWNED by 20261109T130414Z-cloudformation-devops-agent-3-net-new-causes-aws.md and excluded too.

Cause 1 — expected: agentSpaceId is a 36-char lowercase UUID. actual: 'AssertionError: CreateAgentSpace returned an agentSpaceId of length 19 that is not a UUID' / 'CreateAgentSpace returned an agentSpaceId that is not a lowercase UUID' (5 checks).

Cause 2 — expected: CreateAssetFile(path='references/large.md') on a skill asset -> 201, file.version == 2. actual: botocore.errorfactory.ValidationException: An error occurred (ValidationException) when calling the CreateAssetFile operation: Assets of type 'skill' support only one file. Use UpdateAssetFile to replace the existing file. (5 checks).

Cause 3 — expected: CreateAsset(assetType='skill', metadata={'agent_types':['GENERIC']}, content={'zip':{'zipFile': <SKILL.md + references/first.md + references/second.md>}}) -> 201. actual: botocore.errorfactory.ValidationException: An error occurred (ValidationException) when calling the CreateAsset operation: name is required for Skill knowledge items.

Cause 4 — expected: UpdateAssetFile(path='renamed.json') on an artifact holding 'original.json' -> file {path:'renamed.json', version:2}. actual: botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the UpdateAssetFile operation: Asset file not found: renamed.json.

Cause 5 — expected: GetAssetContent(assetVersion=3) on a 2-version asset -> 404 ResourceNotFoundException, message 'Asset not found: <assetId>' (ends with the asset id). actual: 404 ResourceNotFoundException 'Asset not found: 2b130192-9cd8-4a71-8566-fe02952d207a version 3' (the ' version 3' suffix is the divergence; the uuid is just this run's asset id).

Cause 6 — expected: ListAssetFiles(assetVersion=2) before version 2 exists -> 404 ResourceNotFoundException, message 'Asset does not exist'. actual: 404 ResourceNotFoundException 'Asset not found: fbaea577-b6dc-4586-a225-9285886fccfe version 2'.

Cause 7 — expected: ListAssetVersions(nextToken=<never issued>) -> 400 ValidationException 'The provided NextToken is invalid or corrupted.'. actual: 400 ValidationException 'nextToken is not a pagination token issued by this service'.

Cause 8 — expected: GetAsset(...).asset.metadata == {'title': 'service owners', 'status': 'ACTIVE'}. actual: {'status': 'ACTIVE', 'description': 'who answers for each service', 'title': 'service owners'}.

Cause 9 — expected: UpdateAssetFile(path='notes.json', content=<third revision>, clientToken=<token already used for the second revision>) -> 409 ConflictException 'A different request was already submitted with this clientToken.'. actual: HTTP 200 with {'file': {'path': 'notes.json', 'content': {'text': '{"rev": "second"}'}, 'version': 2, ...}} — the cached earlier response, and the submitted content is dropped.

## Steps to Reproduce
Boot the emulator and replay the devops-agent behavioral checks listed in trace_ids. Per cause:
1. CreateAgentSpace(name=<unique>, tags={...}) and inspect agentSpace.agentSpaceId — it is not a lowercase UUID.
2. CreateAsset(assetType='skill', metadata={'name': <n>, 'agent_types': ['GENERIC']}, content={'file': {'path': 'SKILL.md', 'body': {'text': '# x'}}}), then CreateAssetFile(path='references/large.md', content={'text': ...}) — refused with the one-file ValidationException.
3. CreateAsset(assetType='skill', metadata={'agent_types': ['GENERIC']}, content={'zip': {'zipFile': <zip with SKILL.md and two reference files>}}) — refused 'name is required for Skill knowledge items'.
4. CreateAsset(assetType='artifact', content={'file': {'path': 'original.json', ...}}), then UpdateAssetFile(path='renamed.json', content={...}) — refused 404 'Asset file not found: renamed.json'.
5. On a 2-version artifact, GetAssetContent(assetVersion=3) — message carries a ' version 3' suffix.
6. On a 1-version artifact, ListAssetFiles(assetVersion=2) — message is 'Asset not found: <id> version 2' instead of 'Asset does not exist'.
7. ListAssetVersions(nextToken=<forged base64 token>) — wrong ValidationException message.
8. CreateAsset(assetType='artifact', metadata={'title': t, 'description': d}), then GetAsset — metadata still carries 'description'.
9. UpdateAssetFile(path, content=A, clientToken=T) -> version 2; UpdateAssetFile(path, content=A, clientToken=T) -> replay; UpdateAssetFile(path, content=B, clientToken=T) — succeeds instead of 409 ConflictException.
