Issue-Source: trace-sweep-divergence
Issue-Trace: devops-agent-get-asset-content-version-bounds
Issue-Step: setup#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: devops-agent-get-asset-content-version-bounds devops-agent-get-asset-file-input-bounds devops-agent-list-asset-files-paging-bounds devops-agent-list-assets-filter-and-paging-validation devops-agent-agent-space-asset-lifecycle devops-agent-asset-metadata-update-and-type-filter devops-agent-delete-asset-file-path-validation devops-agent-delete-asset-identifier-validation devops-agent-get-asset-id-pattern-and-version-validation
Issue-Shape-Id: com.amazonaws.devopsagent#DevOpsAgent
Issue-Key: devops-agent|CreateAsset|2xx->2xx|$.asset.createdAt
Issue-Key: devops-agent|CreateAsset|2xx->2xx|$.asset.updatedAt
Issue-Key: devops-agent|CreateAsset|2xx->2xx|$.asset.assetId
Issue-Key: devops-agent|GetAsset|2xx->2xx|$.asset.creator
Issue-Key: devops-agent|GetAsset|2xx->2xx|$.asset.editor
Issue-Key: devops-agent|GetAsset|2xx->2xx|$.asset.metadata.title
Issue-Key: devops-agent|GetAsset|2xx->2xx|$.asset.version
Issue-Key: devops-agent|ListAssetFiles|2xx->2xx|$.items[*].createdAt
Issue-Key: devops-agent|ListAssetFiles|2xx->2xx|$.items[*].updatedAt
Issue-Key: devops-agent|GetAssetFile|2xx->2xx|$.file.createdAt
Issue-Key: devops-agent|GetAssetFile|2xx->2xx|$.file.metadata.mimeType
Issue-Key: devops-agent|GetAssetFile|2xx->2xx|$.file.updatedAt
Issue-Key: devops-agent|UpdateAssetFile|2xx->2xx|$.file.createdAt
Issue-Key: devops-agent|UpdateAssetFile|2xx->2xx|$.file.updatedAt
Issue-Commit: 9bfc3dcca28986b1e030db2b6d5b17f08a507141

# devops-agent Asset and AssetFile projections omit creator/editor, misserialize timestamps and return stale metadata

> _Note: this issue was identified against the code base at commit `9bfc3dcca28986b1e030db2b6d5b17f08a507141`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAsset, GetAsset, GetAssetFile, ListAssetFiles, UpdateAssetFile

## Description
Seven net-new wire divergence clusters in devops-agent that share one fault: the Asset / AssetFile / AssetFileSummary structures the emulator projects into asset responses do not match the recorded wire shape — timestamps are emitted as bare epoch-seconds numbers, the creator/editor attribution members are absent, generated asset ids take the wrong form, and a stored asset or asset file is returned with pre-update metadata. Every cluster carries the status AWS returns (201/200); the divergence is the body. A fixer must close all seven before submitting.

=== Shared requirement: asset timestamps ===
AWS: Smithy: Asset$createdAt/$updatedAt, AssetFile$createdAt/$updatedAt and AssetFileSummary$createdAt/$updatedAt are all `Timestamp` and all @required ("Timestamp when this asset/file was created / was last updated"). The model names no explicit format on these members, so recorded behavior is the authority: every recorded asset response carries these members in the same date-time form the model pins explicitly on AgentSpace$createdAt/$updatedAt (@timestampFormat("date-time")), and the corpus's shaped assertion rejects the emulator's numeric value. Hypothesis: the service serializes all resource timestamps uniformly as RFC-3339 date-time strings, and only the AgentSpace members carry the trait that states it.
Emulator: emits a bare epoch-seconds float (e.g. 1795813962.64555) for every one of these members.
Why: an SDK or client that deserializes these members as the recorded date-time form fails or silently mis-reads the value, so no asset's create/update time can be read correctly. The requirement is the serialization form, not any recorded instant.
Fix: the Asset/AssetFile/AssetFileSummary serializers shared by the asset handlers in services/service-com-amazonaws-devopsagent-devopsagent/src/ops/ (create_asset.rs, get_asset.rs, get_asset_file.rs, list_asset_files.rs, update_asset_file.rs) and the timestamp fields on the asset records in src/state.rs.

=== Cause 1 of 7: CreateAsset response timestamps ===
AWS/Emulator/Why/Fix: as the shared requirement above, on CreateAsset POST /asset/agent-space/{agentSpaceId}/assets -> 201, output asset -> Asset (required).
Traces: devops-agent-get-asset-content-version-bounds#setup#2 (cluster also covers setup#2 of devops-agent-get-asset-file-input-bounds, devops-agent-list-asset-files-paging-bounds and devops-agent-list-assets-filter-and-paging-validation).

=== Cause 2 of 7: ListAssetFiles item timestamps ===
AWS: Smithy: ListAssetFiles -> 200, output items -> AssetFileSummaryList of AssetFileSummary {path, metadata, version, createdAt, updatedAt}; createdAt/updatedAt required, recorded in the date-time form.
Emulator: each listed item carries epoch-seconds floats for createdAt/updatedAt.
Why/Fix: as the shared requirement; here in the ListAssetFiles item projection.
Traces: devops-agent-delete-asset-file-path-validation#task#5 (cluster also covers devops-agent-list-asset-files-paging-bounds#task#3 and #task#6).

=== Cause 3 of 7: UpdateAssetFile response timestamps ===
AWS: Smithy: UpdateAssetFile PATCH .../files/{path+} -> 200, output file -> AssetFile (required) with required createdAt/updatedAt.
Emulator: both members are epoch-seconds floats (and they differ from each other, so the record is stamped correctly — only the serialization form is wrong).
Traces: devops-agent-get-asset-content-version-bounds#task#5.

=== Cause 4 of 7: GetAsset omits Asset creator attribution ===
AWS: Smithy: GetAsset -> 200, output asset -> Asset (required). The checked-in model lists no creator/editor member on Asset, so recorded behavior only: the recorded GetAsset response carries an `asset.creator` object holding the identity that created the asset (a `uid` plus a `displayName` naming the calling principal's kind, e.g. the IAM role user the request was signed as). The uid is a per-run value and is not asserted; the MEMBER's presence and shape are.
Emulator: `asset.creator` is absent from the 200 response.
Why: a client that shows or filters assets by who created them has nothing to read, so attribution is lost for every asset the emulator serves.
Fix: the Asset projection in src/ops/get_asset.rs plus the creator identity recorded on the asset in src/ops/create_asset.rs / src/state.rs — record the calling principal on create and project it as `creator`.
Traces: devops-agent-delete-asset-identifier-validation#task#5 (cluster also covers devops-agent-get-asset-id-pattern-and-version-validation#task#3).

=== Cause 5 of 7: GetAsset omits creator AND editor and reports pre-update metadata and version ===
AWS: Recorded behavior only (Asset$metadata is a Document, so the model names no key): after an asset's metadata is updated, GetAsset reports the UPDATED metadata, `asset.version` advanced to 2, and both an `asset.creator` and an `asset.editor` object (uid plus displayName of the principal that last edited it).
Emulator: GetAsset answers 200 but still reports the pre-update metadata and version 1, and carries neither creator nor editor. The stale metadata/version is downstream of the update operation itself not being implemented — the open issue 20261127T091917Z-devops-agent-updateasset-stub.md owns the UpdateAsset scaffold stub — but this read-side cluster is not reported as owned by it, and the missing creator/editor members are independent of it.
Why: a client that updates an asset and reads it back sees its write silently lost and cannot tell who last edited the asset.
Fix: once UpdateAsset is implemented it must bump Asset$version and replace the stored metadata; independently, the GetAsset projection must carry creator and editor.
Traces: devops-agent-asset-metadata-update-and-type-filter#task#5.

=== Cause 6 of 7: GetAssetFile returns pre-update file metadata (plus the timestamp form) ===
AWS: Smithy: GetAssetFile -> 200, output file -> AssetFile {path, content, metadata (Document), version, createdAt, updatedAt}; UpdateAssetFile PATCH takes a `metadata` Document. Recorded behavior: after an UpdateAssetFile that supplies new file metadata, GetAssetFile reports that new metadata — in the recorded flow the file's declared content type becomes the newly written one.
Emulator: GetAssetFile still reports the metadata the file was created with (the original content type) and emits epoch-seconds timestamps, so the metadata written by UpdateAssetFile is dropped.
Why: a client that re-publishes a file with new metadata reads back the old metadata and believes nothing changed.
Fix: the UpdateAssetFile write path (src/ops/update_asset_file.rs) and the asset-file record in src/state.rs — store the submitted metadata on the file version and project it from GetAssetFile.
Traces: devops-agent-delete-asset-file-path-validation#task#6 (cluster also covers devops-agent-get-asset-content-version-bounds#task#6, #task#7 and devops-agent-get-asset-file-input-bounds#task#3).

=== Cause 7 of 7: CreateAsset mints an assetId of the wrong form ===
AWS: Smithy: Asset$assetId -> ResourceId, @required, length 1..128, @pattern ^[a-zA-Z0-9_.-]+$ — permissive. Recorded behavior only: for this flow the recorded CreateAsset response carries an assetId whose form the corpus's shaped assertion pins, and the emulator's generated value is of a different form (it returns a random UUID). The recorded id value itself is not asserted and is not a requirement; its form is.
Emulator: CreateAsset returns assetId as a random 36-character UUID where the recorded response for this request carries an id of another form, so the shape assertion fails on the create response. Hypothesis: the service derives the asset id from the request (the asset type and/or the submitted content/metadata name) rather than minting a random one, which is also why the same assertion passes for the CreateAsset calls in cause 1.
Why: a client that shape-checks or parses the asset id it gets back rejects or mis-stores the asset.
Fix: the asset-id minting in src/ops/create_asset.rs / the asset id generator in src/state.rs — produce the id form the recorded responses carry for the asset type being created.
Traces: devops-agent-agent-space-asset-lifecycle#task#2.

## Evidence
Replay of simparity-wire-aws-v1; expected and actual status agree in all seven causes.

Cause 1 — devops-agent-get-asset-content-version-bounds#setup#2, CreateAsset, 201 -> 201:
  $.asset.createdAt: shape mismatch (got 1795813962.64555); $.asset.updatedAt: shape mismatch (got 1795813962.64555). 4 fixture steps in cluster.

Cause 2 — devops-agent-delete-asset-file-path-validation#task#5, ListAssetFiles, 200 -> 200:
  $.items[0].createdAt / $.items[0].updatedAt: shape mismatch (got 1795813976.174059). 3 fixture steps in cluster.

Cause 3 — devops-agent-get-asset-content-version-bounds#task#5, UpdateAssetFile, 200 -> 200:
  $.file.createdAt: shape mismatch (got 1795813962.64555); $.file.updatedAt: shape mismatch (got 1795813962.66534).

Cause 4 — devops-agent-delete-asset-identifier-validation#task#5, GetAsset, 200 -> 200:
  $.asset.creator: missing (expected an object with a `uid` member, got None). 2 fixture steps in cluster.

Cause 5 — devops-agent-asset-metadata-update-and-type-filter#task#5, GetAsset, 200 -> 200:
  $.asset.metadata.title: value_mismatch (expected the post-update title, got the pre-update title — replay shows expected 'after update', got 'before update')
  $.asset.version: value_mismatch (expected 2, got 1)
  $.asset.creator: missing (expected {'uid': <per-run>, 'displayName': 'IAM role user'}, got None)
  $.asset.editor: missing (expected {'uid': <per-run>, 'displayName': 'IAM role user'}, got None)

Cause 6 — devops-agent-delete-asset-file-path-validation#task#6, GetAssetFile, 200 -> 200:
  $.file.metadata.mimeType: value_mismatch (expected the type written by the update, 'application/json', got the original 'text/markdown')
  $.file.createdAt / $.file.updatedAt: shape mismatch (got 1795813976.174059). 4 fixture steps in cluster.

Cause 7 — devops-agent-agent-space-asset-lifecycle#task#2, CreateAsset, 201 -> 201:
  $.asset.assetId: shape mismatch (expected the recorded id form, got 'b7607515-8a68-433c-879a-54d531375404').

All epoch numbers, uids and ids quoted above are this run's values and are not themselves requirements.

## Steps to Reproduce
Boot the emulator from this checkout and replay the fixture steps named per cause. By hand, in a live agent space:
1. POST /asset/agent-space/{agentSpaceId}/assets — inspect asset.createdAt/updatedAt (expect the recorded date-time form, not a float) and asset.assetId (expect the recorded id form).
2. GET .../assets/{assetId}/files — inspect items[*].createdAt/updatedAt.
3. PATCH .../assets/{assetId}/files/{path} — inspect file.createdAt/updatedAt.
4. GET .../assets/{assetId} — expect an `asset.creator` object.
5. Update an asset's metadata, then GET .../assets/{assetId} — expect the new metadata, version 2, and both `creator` and `editor`.
6. PATCH .../assets/{assetId}/files/{path} with new metadata, then GET the same path — expect the newly written metadata.
