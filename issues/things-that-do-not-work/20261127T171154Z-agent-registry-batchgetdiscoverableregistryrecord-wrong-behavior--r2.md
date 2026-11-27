Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: eca447fb6703ac098f99a805138e6e6e630c3cd7

# RegistryRecordSummary$artifact is hardcoded absent on every record, on the false premise that the owning plane stores no skill package

_Filed: 2026-11-27T17:11:54Z_
_Operation: BatchGetDiscoverableRegistryRecord | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `eca447fb6703ac098f99a805138e6e6e630c3cd7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`RegistryRecordSummary$artifact` is the one member the model says ONLY this operation returns, and its doc says it is "[a]bsent when the record has no skill package" — i.e. present when the record has one. The crate hardcodes `artifact: None` for every record and justifies it with a claim about the owning service that is false: the Agent Registry control plane DOES accept a skill package on `CreateRegistryRecord` (`artifact` of type RegistryRecordArtifactUpload), stores its checksum as `RegistryRecord::artifact_sha256`, and already reports it through a helper (`crate::state::artifact_download`).

Two places drop it: `owner::record` projects the owner's record member by member and never carries `artifact_sha256` over (the discovery `RegistryRecord` struct has no field for it), and `RegistryRecord::summary()` then writes the literal `artifact: None`. So a caller that uploads a package through the control plane, has the record approved, and then batch-gets it, is told the record has no skill package — a hollow value computed from nothing rather than from stored state, while `GetRegistryRecord` on the same record reports the checksum.

Note the second half of the model sentence, which the shared projection also contradicts: `summary()` is used by BOTH reads and documents itself as rendering "identically in either response", whereas the model says `artifact` is returned by BatchGetDiscoverableRegistryRecord and NOT by SearchDiscoverableRegistryRecords — so the member has to be decided per operation, not in the one shared conversion.

## Evidence

Smithy, `RegistryRecordSummary$artifact` (smithy_query(query="shape", shape="RegistryRecordSummary")):
  "The skill package of the record's latest approved revision, as the package's SHA-256 checksum and a download URL that is valid for up to 5 minutes. Only BatchGetDiscoverableRegistryRecord returns this member, not SearchDiscoverableRegistryRecords. Absent when the record has no skill package."
(Both members of `RegistryRecordArtifactDownload` — `sha256`, `downloadUrl` — are optional, so a checksum-only answer is well formed.)

The false premise, services/service-com-amazonaws-agentregistry-agentregistry/src/state.rs:294-305:
  /// The full `RegistryRecordSummary` — what `BatchGetDiscoverableRegistryRecord`
  /// and `SearchDiscoverableRegistryRecords` return. Both go through this one
  /// conversion, so the same record renders identically in either response.
  ...
  /// `artifact` ... is absent on every record: its own doc says the member is "[a]bsent when the record
  /// has no skill package", and no record ever has one, because the plane that owns
  /// registry records binds no operation that attaches a package to a record (its
  /// model declares no package member and no upload at all,
  /// `services/service-com-amazonaws-agentregistrycontrol-agentregistrycontrol/OPERATIONS.md`).
and the hardcoded value at src/state.rs:321:
            artifact: None,

The cited OPERATIONS.md says the opposite — services/service-com-amazonaws-agentregistrycontrol-agentregistrycontrol/OPERATIONS.md:123 (CreateRegistryRecord input):
  ... `descriptors` (Descriptors, required), `artifact` (RegistryRecordArtifactUpload), ...
and :132 (GetRegistryRecord output): ... `artifact` (RegistryRecordArtifactDownload) ...

The owner stores and reports it:
  services/service-com-amazonaws-agentregistrycontrol-agentregistrycontrol/src/state.rs:618-621 — "/// The SHA-256 checksum of the skill package uploaded with this record ... pub artifact_sha256: Option<String>,"
  .../src/ops/create_registry_record.rs:479-482 — "artifact_sha256: input.artifact.as_ref().map(crate::state::stored_artifact_checksum),"
  .../src/ops/get_registry_record.rs:262-265 — "artifact: record.artifact_sha256.as_deref().map(crate::state::artifact_download),"
  .../src/state/artifact.rs:74-79 — "pub fn artifact_download(sha256: &str) -> RegistryRecordArtifactDownload { ... sha256: Some(...), download_url: None }"

This crate's own test file proves the member exists on the owner's record it seeds — tests/all/discovery_read_path.rs:107-109:
  // A seeded record carries no uploaded skill package: the discovery surface
  // reports no member of one, so no row of these tests varies over it.
  artifact_sha256: None,

Where it is dropped in projection: src/state/owner.rs:154-194 (`fn record(...)`) rebuilds every published member but never reads `held.artifact_sha256`; the discovery record struct (src/state.rs:243-266) declares no field for it.

Test coverage: tests/all/discovery_read_path.rs:2417-2457 (`no_discovery_answer_carries_a_package_or_a_signature_card_no_store_holds`) asserts `record.get("artifact").is_none()` for records seeded with `artifact_sha256: None`, and its stated reason — "its model declares neither" — is the same false premise; the assertion itself still holds after the fix, since those records genuinely carry no package.

## Steps to Reproduce

1. Through AgentRegistryControl call CreateRegistryRecord with an `artifact` upload (a skill package) on a registry whose approval config auto-approves, and submit the record so it becomes APPROVED.
2. Call GetRegistryRecord on it: the response carries `artifact.sha256` (the checksum of the uploaded bytes), proving the store holds a skill package for the record.
3. Call BatchGetDiscoverableRegistryRecord with `entries=[{registryId, recordIds:[<that record id>]}]`.
4. Observe HTTP 200 with the record summary carrying NO `artifact` member.
5. Expected per `RegistryRecordSummary$artifact`: the member is present (absent only "when the record has no skill package"), and it is returned by this operation only — SearchDiscoverableRegistryRecords must keep omitting it, which the single shared `RegistryRecord::summary()` cannot express today.
