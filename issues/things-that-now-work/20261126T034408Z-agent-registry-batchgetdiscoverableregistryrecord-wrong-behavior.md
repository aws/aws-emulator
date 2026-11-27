Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 41be3a2659be45277be7049b64517ef95e5ebd6f

# RegistryRecordSummary$customMetadata is returned for records in a registry with no custom metadata schema configured

_Filed: 2026-11-26T03:44:08Z_
_Operation: BatchGetDiscoverableRegistryRecord | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `41be3a2659be45277be7049b64517ef95e5ebd6f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model states the presence condition for `customMetadata` on `RegistryRecordSummary`: the field "is only present if the registry has a custom metadata schema configured". This crate's projection copies a record's stored metadata into every `RegistryRecordSummary` unconditionally and never looks at the owning registry's `customMetadataSchemaConfiguration` — the string does not appear anywhere in this crate. The state is reachable through the emulator's own operations: the Agent Registry control plane accepts `customMetadata` on `CreateRegistryRecord` for a registry that declares no schema (the schema evaluation answers `Compliant` when there is no configuration to violate), so a record in a schema-less registry can hold metadata, and `BatchGetDiscoverableRegistryRecord` (and `SearchDiscoverableRegistryRecords`, which renders through the same `RegistryRecord::summary()`) publishes it. The registry is in hand at the projection site, so the condition is decidable where the member is written.

## Evidence

The model sentence — smithy_query(query="doc", shape="RegistryRecordSummary", member="customMetadata"):
```
The custom metadata attached to this registry record. Values are strings (maximum 128
characters) or booleans. This field is only present if the registry has a custom
metadata schema configured.
```

The unconditional projection — services/service-com-amazonaws-agentregistry-agentregistry/src/state.rs:220-235, in `RegistryRecord::summary()` ("what `BatchGetDiscoverableRegistryRecord` and `SearchDiscoverableRegistryRecords` return"):
```
            custom_metadata: self.custom_metadata.clone(),
```
and where that value comes from, services/service-com-amazonaws-agentregistry-agentregistry/src/state/owner.rs:124-126:
```
        custom_metadata: published
            .map_or(&held.custom_metadata, |revision| &revision.custom_metadata)
            .clone(),
```
`discoverable(held: &control::Registry)` (owner.rs:71-81) holds the whole owning registry, yet nothing in this crate reads `custom_metadata_schema_configuration` (grep over the crate: no match; the only matches in the repo are in the control-plane crate).

That a schema-less registry can hold records carrying metadata — services/service-com-amazonaws-agentregistrycontrol-agentregistrycontrol/src/state/custom_metadata.rs:167-171:
```
    pub fn verdict(&mut self, record_type: &str, metadata: Option<&Document>) -> SchemaVerdict {
        let Some(configuration) = self.configuration else {
            // The registry constrains nothing, so there is nothing the metadata can violate.
            return SchemaVerdict::Compliant;
```

Existing coverage pins the current (contradicting) output: the seeded registry declares no schema — services/service-com-amazonaws-agentregistry-agentregistry/tests/all/discovery_read_path.rs:170 `custom_metadata_schema_configuration: None,` — the record carries metadata (:217-220 `.with_custom_metadata(document_object(&[("environment", ...), ("requiresApproval", ...)]))`), and the assertion at :979-981 is `from_batch["customMetadata"] ... "BatchGet must return the stored customMetadata"` with no reason stated for ignoring the registry's configuration.

## Steps to Reproduce

1. Through the control plane, CreateRegistry with no customMetadataSchemaConfiguration, then CreateRegistryRecord in it with customMetadata {"environment": "production"} (accepted: custom_metadata.rs:167-171 returns Compliant), and approve the record so it becomes discoverable.
2. POST /discoverable-records-batch with {"entries": [{"registryId": "<that registry>", "recordIds": ["<that record>"]}]}.
3. Observe the returned RegistryRecordSummary carries `customMetadata`, although the registry has no custom metadata schema configured and the member's doc says the field "is only present if the registry has a custom metadata schema configured".
4. The same body is returned by POST /discoverable-records-search for the same record, since both render through RegistryRecord::summary() (state.rs:220-235).
