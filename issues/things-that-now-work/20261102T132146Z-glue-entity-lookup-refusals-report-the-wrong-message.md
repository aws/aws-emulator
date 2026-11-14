Issue-Source: trace-sweep-divergence
Issue-Trace: glue-cflog-database-and-table-name-forms
Issue-Step: teardown#10
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: glue-cflog-database-and-table-name-forms glue-cflog-table-before-database-then-duplicate-retry glue-tsd-partition-and-version-side-reads glue-gpn-traj-job-runs-for-a-job-that-never-ran glue-gjwr-traj-catalog-is-regional-under-one-catalog-id glue-tsd-schema-registry-confusion
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue|GetDatabase|4xx->4xx|$.Message
Issue-Key: glue|GetTableVersions|4xx->4xx|$.Message
Issue-Key: glue|GetJobRun|4xx->4xx|$.Message
Issue-Key: glue|GetDatabase|4xx->4xx|$.__type
Issue-Key: glue|GetSchema|4xx->4xx|$.Message
Issue-Key: glue|GetSchema|4xx->4xx|$.__type
Issue-Related: 20261102T132146Z-glue-catalog-input-validation-returns-validationexception-instead-of.md
Issue-Commit: c588c92daf57694e44caee6ddcc17c51be576a2d

# Glue entity-lookup refusals report the wrong message text and the wrong error code

> _Note: this issue was identified against the code base at commit `c588c92daf57694e44caee6ddcc17c51be576a2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDatabase

## Description
Seven Glue not-found / bad-reference refusals return the right HTTP status but the wrong message text, and two of them also return the wrong error code. Each operation has its own fixed sentence in Glue; the emulator substitutes a generic one, interpolates values Glue does not interpolate (or fails to interpolate the ones it does), or classifies the fault as a different error entirely. Seven causes.

Cause 1 of 7: GetDatabase's not-found message does not lower-case the database name. AWS: Smithy Table/Database names are documented "For Hive compatibility, this must be entirely lowercase" — Glue folds catalog names to lowercase, so the refusal for a mixed-case request reads 'Database <name> not found.' with <name> being the LOWERCASED form of the requested Name. Emulator: echoes the request's mixed-case spelling verbatim. Why: a client matching the message against the canonical (stored) name mismatches, and the response implies Glue preserved the case it actually folded. Fix: GetDatabase's not-found path must render the canonicalised name. Traces: glue-cflog-database-and-table-name-forms#teardown#10.

Cause 2 of 7: GetTable with an absent DATABASE uses a request-derived sentence. AWS: Smithy GetTable models EntityNotFoundException (httpError 400); the recording shows `Message` 'Entity Not Found' — entirely fixed text with no name interpolated. Emulator: 'Database <name> not found.', built from the request. Fix: GetTable's absent-database path. Note open issue 20261101T211008Z-glue-gettable-reports-an-absent-database-with-the.md describes this same emulator behaviour but does not carry this fixture step. Traces: glue-cflog-table-before-database-then-duplicate-retry#teardown#11.

Cause 3 of 7: GetTableVersions' absent-table refusal is the generic sentence. AWS: Smithy GetTableVersions models EntityNotFoundException (httpError 400); the recording shows `Message` 'Table <tableName> not found.' — fixed text 'Table ' + the request's TableName + ' not found.'. Emulator: 'Entity Not Found'. Why: the inverse of cause 2 — the emulator uses the generic text where Glue names the table and names the database where Glue is generic, so neither message can be relied on. Fix: GetTableVersions' not-found path. Traces: glue-tsd-partition-and-version-side-reads#task#7.

Cause 4 of 7: GetJobRun for an existing job but an unknown RunId reports the JOB as missing. AWS: Smithy GetJobRun models EntityNotFoundException (httpError 400); the recording shows `Message` 'Job run not found for the given job name and runId.' — entirely fixed text. Emulator: 'Job not found.', which blames the job. Why: the client retries or recreates the job when the real fault is a stale run id. Fix: GetJobRun's lookup must distinguish an absent job from an absent run. Traces: glue-gpn-traj-job-runs-for-a-job-that-never-ran#task#3.

Cause 5 of 7: GetDatabase against an unknown CatalogId returns AccessDeniedException instead of EntityNotFoundException. AWS: Smithy GetDatabase models EntityNotFoundException (httpError 400); the recording shows `__type` EntityNotFoundException with `Message` 'Catalog not found.' — entirely fixed text. Emulator: 400 with `__type` AccessDeniedException and an IAM-shaped sentence of the form 'User: <principal arn> is not authorized to perform: glue:GetDatabase on resource: <arn> because no resource-based policy allows the glue:GetDatabase action'. Why: the error code is wrong AND it mis-attributes a missing catalog to an authorization failure, so a client adds IAM policy to fix a non-existent catalog. Fix: GetDatabase's catalog resolution must raise EntityNotFoundException before any authorization decision. Traces: glue-gjwr-traj-catalog-is-regional-under-one-catalog-id#task#5 (cluster also covers #task#6).

Cause 6 of 7: GetSchema with neither SchemaArn nor registryName+schemaName returns a not-found instead of a validation refusal. AWS: Smithy GetSchema.SchemaId is required and models both InvalidInputException and EntityNotFoundException (httpError 400 each); the recording shows `__type` InvalidInputException with `Message` 'At least one of (registryName and schemaName) or schemaArn has to be provided.' — entirely fixed text. Emulator: `__type` EntityNotFoundException with 'Schema is not found.'. Why: the request is malformed, not pointing at a missing schema; the client is told to create a schema instead of to fill in SchemaId. Fix: GetSchema's SchemaId validation, ahead of the registry lookup. Traces: glue-tsd-schema-registry-confusion#task#4.

Cause 7 of 7: GetSchema for a schema in a non-existent registry reports the schema missing rather than the registry. AWS: the recording shows `Message` 'Registry is not found. RegistryName: <registryName>' — fixed text 'Registry is not found. RegistryName: ' + the request's SchemaId.RegistryName. Emulator: 'Schema is not found.'. Why: the client cannot tell that the registry itself is absent, so it creates a schema in a registry that does not exist. Fix: GetSchema must resolve the registry before the schema and report the registry when it is absent. Traces: glue-tsd-schema-registry-confusion#task#5.

Related open issue (does not cover these steps): 20261029T201859Z-glue-not-found-refusals-carry-emulator-authored-messages.md.

## Evidence
Replay of trace set simparity-wire-aws-v1, service glue. All seven clusters are 400/400 with diverging body.

Cause 1 — glue-cflog-database-and-table-name-forms#teardown#10 GetDatabase: $.Message expected 'Database sim2real_probe_cflog_nm_983252bd not found.' (lowercased), got 'Database Sim2Real_Probe_CfLog_Nm_983252bd not found.'.
Cause 2 — glue-cflog-table-before-database-then-duplicate-retry#teardown#11 GetTable: $.Message expected 'Entity Not Found', got 'Database sim2real_probe_cflog_ord_73346cf3 not found.'.
Cause 3 — glue-tsd-partition-and-version-side-reads#task#7 GetTableVersions: $.Message expected 'Table andes_3_0_dta not found.', got 'Entity Not Found'.
Cause 4 — glue-gpn-traj-job-runs-for-a-job-that-never-ran#task#3 GetJobRun: $.Message expected 'Job run not found for the given job name and runId.', got 'Job not found.'.
Cause 5 — glue-gjwr-traj-catalog-is-regional-under-one-catalog-id#task#5 GetDatabase: $.__type expected 'EntityNotFoundException', got 'AccessDeniedException'; $.Message expected 'Catalog not found.', got the 'User: ... is not authorized to perform: glue:GetDatabase on resource: ... because no resource-based policy allows the glue:GetDatabase action' sentence. 2 steps in cluster.
Cause 6 — glue-tsd-schema-registry-confusion#task#4 GetSchema: $.__type expected 'InvalidInputException', got 'EntityNotFoundException'; $.Message expected 'At least one of (registryName and schemaName) or schemaArn has to be provided.', got 'Schema is not found.'.
Cause 7 — glue-tsd-schema-registry-confusion#task#5 GetSchema: $.Message expected 'Registry is not found. RegistryName: sim2real-probe-tsd-reg-28d39fc6', got 'Schema is not found.'.

## Steps to Reproduce
Replay the wire corpus and inspect these glue steps:
1. glue-cflog-database-and-table-name-forms#teardown#10 — GetDatabase with a mixed-case name for an absent database.
2. glue-cflog-table-before-database-then-duplicate-retry#teardown#11 — GetTable whose database does not exist.
3. glue-tsd-partition-and-version-side-reads#task#7 — GetTableVersions for an absent table.
4. glue-gpn-traj-job-runs-for-a-job-that-never-ran#task#3 — GetJobRun with an unknown RunId on an existing job.
5. glue-gjwr-traj-catalog-is-regional-under-one-catalog-id#task#5 — GetDatabase with an unknown CatalogId.
6. glue-tsd-schema-registry-confusion#task#4 — GetSchema with an empty SchemaId.
7. glue-tsd-schema-registry-confusion#task#5 — GetSchema naming a non-existent registry.
