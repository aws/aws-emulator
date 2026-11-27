Issue-Source: trace-sweep-divergence
Issue-Trace: glue-schema-registry-diff-and-metadata-removal
Issue-Step: task#8
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: glue-schema-registry-diff-and-metadata-removal glue-schema-registry-versioning
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue|PutSchemaVersionMetadata|2xx->2xx|$.LatestVersion
Issue-Key: glue|PutSchemaVersionMetadata|2xx->2xx|$.VersionNumber
Issue-Key: glue|RemoveSchemaVersionMetadata|2xx->2xx|$.LatestVersion
Issue-Key: glue|RemoveSchemaVersionMetadata|2xx->2xx|$.VersionNumber
Issue-Key: glue|GetSchema|2xx->2xx|$.SchemaCheckpoint
Issue-Commit: 489c1197152cf1b4a01399a5eda62086fe4d180a

# Glue schema-registry version bookkeeping: metadata writes and GetSchema report the registry head instead of the addressed version

> _Note: this issue was identified against the code base at commit `489c1197152cf1b4a01399a5eda62086fe4d180a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutSchemaVersionMetadata

## Description
This bundle carries 3 causes, all in the Glue schema-registry version bookkeeping: every one of them answers a question about ONE addressed schema version (or about the schema's checkpoint) with the registry's CURRENT HEAD version instead. A fixer must close all 3 before submitting.

Cause 1 of 3 — PutSchemaVersionMetadata reports the head version, not the version the metadata was attached to.
AWS: Smithy (service-com-amazonaws-glue-awsglue): PutSchemaVersionMetadata output members LatestVersion (target LatestSchemaVersionBoolean, @default false, doc "The latest version of the schema.") and VersionNumber (target VersionLongNumber). The recording shows AWS answering 200 and describing the version the request ADDRESSED: for a request that targets a version which is not the schema's head, LatestVersion is false and VersionNumber is 0. Hypothesis: AWS only fills VersionNumber from the request's SchemaVersionNumber, so a request that identifies the version by SchemaVersionId leaves it at 0 and LatestVersion at its modelled default false. Recorded behavior only for the 0 value — the model gives VersionLongNumber a @range min of 1, so the zero is not derivable from the model.
Emulator: 200, but LatestVersion is reported true and VersionNumber is the registry's current head version number (the replayed run reported LatestVersion=true, VersionNumber=2 where AWS sent false/0).
Why: a client that attaches metadata to an older version and keys off LatestVersion/VersionNumber to confirm WHICH version it just annotated is told it annotated the head. Schema-evolution tooling that only promotes/checkpoints when LatestVersion is true will act on the wrong version, and a client comparing the echoed VersionNumber with the one it asked for sees a false match.
Fix: the PutSchemaVersionMetadata handler's response projection in the Glue schema-registry component — build LatestVersion/VersionNumber from the resolved target version of the request (and leave VersionNumber unset/0 when the request addressed the version by SchemaVersionId) instead of reading the registry's head.
Traces: glue-schema-registry-diff-and-metadata-removal#task#8 (cluster also covers glue-schema-registry-versioning#task#12).

Cause 2 of 3 — RemoveSchemaVersionMetadata has the same head-version projection.
AWS: Smithy: RemoveSchemaVersionMetadata output carries the same LatestVersion (@default false) and VersionNumber (VersionLongNumber) members as the Put form; the recording shows the removal answered 200 with LatestVersion false and VersionNumber 0 for a non-head addressed version, i.e. the response again describes the addressed version.
Emulator: 200 with LatestVersion=true and VersionNumber=<registry head> (replayed run: true / 2).
Why: identical client damage to cause 1 on the removal path — a tool that strips metadata from a specific historical version is told it operated on the head version, so an audit/reconciliation loop that re-reads the version it was told about touches the wrong one.
Fix: the RemoveSchemaVersionMetadata handler's response projection; it should share the addressed-version resolution with PutSchemaVersionMetadata rather than each reading registry head.
Traces: glue-schema-registry-diff-and-metadata-removal#task#9.

Cause 3 of 3 — GetSchema's SchemaCheckpoint tracks the latest version instead of the checkpoint.
AWS: Smithy: GetSchemaResponse$SchemaCheckpoint (target SchemaCheckpointNumber, @range 1..100000), documented as "The version number of the checkpoint (the last time the compatibility mode was changed)." It is therefore a distinct value from LatestSchemaVersion/NextSchemaVersion: registering further schema versions does NOT move it; only a compatibility-mode change does. The recording shows a schema with several registered versions still reporting the checkpoint at the version it was last set at.
Emulator: 200, but SchemaCheckpoint equals the schema's latest registered version number (replayed run reported 2 where AWS reported 1), so the member is just an alias of LatestSchemaVersion.
Why: a client that uses SchemaCheckpoint to decide whether the compatibility mode has moved since it last read the schema sees a change on every PUT of a new version, and conversely can never detect a real checkpoint change; schema-evolution gates keyed on checkpoint-vs-latest drift always see them equal.
Fix: the Glue schema-registry state for a schema should hold the checkpoint version separately from the latest-version counter, advanced only when the compatibility mode is changed, and GetSchema's projection should read that field.
Traces: glue-schema-registry-versioning#task#15.

## Evidence
Cause 1 (PutSchemaVersionMetadata, glue-schema-registry-diff-and-metadata-removal#task#8, status 200 both sides): expected $.LatestVersion=false, $.VersionNumber=0; actual $.LatestVersion=true, $.VersionNumber=2. Same cluster: glue-schema-registry-versioning#task#12.
Cause 2 (RemoveSchemaVersionMetadata, glue-schema-registry-diff-and-metadata-removal#task#9, status 200 both sides): expected $.LatestVersion=false, $.VersionNumber=0; actual true / 2.
Cause 3 (GetSchema, glue-schema-registry-versioning#task#15, status 200 both sides): expected $.SchemaCheckpoint=1; actual 2 (equal to the schema's latest registered version).
The concrete numbers above are one run's values; the requirement is that the metadata responses describe the ADDRESSED version and that SchemaCheckpoint is the compatibility checkpoint, not the head version.

## Steps to Reproduce
1. CreateRegistry + CreateSchema, then RegisterSchemaVersion twice so the schema has more than one version and a head above 1.
2. Call PutSchemaVersionMetadata addressing a NON-head version (by SchemaVersionId) with a MetadataKeyValue pair -> emulator answers LatestVersion=true and VersionNumber=<head>; AWS answers LatestVersion=false and VersionNumber=0.
3. Call RemoveSchemaVersionMetadata for the same key on the same non-head version -> same divergence.
4. Call GetSchema for the schema without ever changing its compatibility mode -> emulator reports SchemaCheckpoint equal to the latest version number; AWS reports the checkpoint version (unchanged since the compatibility mode was last set).
Replay: glue-schema-registry-diff-and-metadata-removal (task#8, task#9) and glue-schema-registry-versioning (task#12, task#15).
