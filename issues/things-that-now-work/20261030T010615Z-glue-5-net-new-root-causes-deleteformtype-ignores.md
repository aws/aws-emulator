Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue.delete-form-type-referenced-by-asset-type-fails glue.form-type-delete-referenced-by-asset-type-rejected glue.create-trigger-conditional-without-predicate-fails glue.delete-column-statistics-task-settings-nonexistent-not-found glue.column-statistics-task-settings-duplicate-create-rejected glue.partition-create-get-delete glue.e2e-data-quality-ruleset-evaluation-flow
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue.delete-form-type-referenced-by-asset-type-fails|run#22|AssertionError|assert conflict == ("ConflictException", conflict_message), (
Issue-Key: glue.form-type-delete-referenced-by-asset-type-rejected|run#21|AssertionError|assert conflict_code == "ConflictException", f"unexpected error code: {conflict_code}"
Issue-Key: glue.create-trigger-conditional-without-predicate-fails|run#19|AssertionError|assert get_message == "Trigger not found.", f"unexpected not-found message {get_message!r}"
Issue-Key: glue.delete-column-statistics-task-settings-nonexistent-not-found|run#15|AssertionError|assert code == "EntityNotFoundException", (
Issue-Key: glue.column-statistics-task-settings-duplicate-create-rejected|run#19|AssertionError|assert code == "EntityNotFoundException", f"Delete on a table with no settings: expected EntityNotFoundException, got {code}"
Issue-Key: glue.partition-create-get-delete|run#19|AssertionError|assert update_code == "EntityNotFoundException", (
Issue-Key: glue.e2e-data-quality-ruleset-evaluation-flow|_start_evaluation_with_retry#18|Timeout|time.sleep(5)
Issue-Commit: 557cb2f2dcf6ed4bc23d8b47d317a66a9b7641c1

# Glue: 5 net-new root causes — DeleteFormType ignores asset-type references, GetTrigger's not-found message is emulator-authored, and three sub-resource paths answer NotImplemented/never execute

> _Note: this issue was identified against the code base at commit `557cb2f2dcf6ed4bc23d8b47d317a66a9b7641c1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteFormType

## Description
This issue bundles 5 net-new root causes in the `glue` service, all surfaced by the behavioral suite (simparity-behavioral-aws-v1). They are independent faults and each must be closed on its own before this issue is submitted. Causes are numbered 1..5 below, each with its own failing check ids.

Cause 1 of 5: DeleteFormType deletes a metadata form type that is still referenced by an asset type, instead of refusing with ConflictException.
AWS: Smithy: `DeleteFormType` (DELETE /form-types/{Identifier}; httpBindings inert under the awsJson RPC protocol) models `ConflictException` (errorKind client, httpError 400) among its errors. Real Glue runs a live reference check: while an asset type (created by PutAssetType with `Forms: {<formName>: {FormTypeIdentifier: <formName>}}`) still references the form type, DeleteFormType is refused with HTTP 400 / `ConflictException`, and the Message is the fixed text `Form type <formTypeName> cannot be deleted because of the following dependencies: gluetype glueAsset#glue_<accountId>#<assetTypeName>` wrapped by Glue in a JSON envelope — i.e. the Message value is literally `{"Message":"Form type <formTypeName> cannot be deleted because of the following dependencies: gluetype glueAsset#glue_<accountId>#<assetTypeName>"}`. Fixed parts: `Form type `, ` cannot be deleted because of the following dependencies: gluetype glueAsset#glue_`, `#`. Variable parts, all taken from the request and from stored state: the form type Identifier from the request, the caller's account id, and the name of the referencing asset type. The guard is a live reference check and not a permanent lock: once the referencing asset (DeleteAsset) and asset type (DeleteAssetType) are removed, the identical DeleteFormType call succeeds, and GetFormType then refuses with `EntityNotFoundException` / Message `{"Message":"Metadata form type <formTypeName> not found."}`.
Emulator: the delete is ACCEPTED. `glue.delete-form-type-referenced-by-asset-type-fails` records the refusal probe returning `('<succeeded>', '')` where `('ConflictException', <the message above>)` was required; `glue.form-type-delete-referenced-by-asset-type-rejected` polls repeatedly for the conflict to be enforced and ends with `conflict_code == None`, i.e. every attempt succeeded. No reference index is consulted at all, so the referenced form type is destroyed.
Why: a client that relies on this guard loses the form type that its asset type and its stored assets depend on, and the asset type is left pointing at a form type that no longer exists. The negative path is also unobservable, so a caller cannot distinguish "safe to delete" from "still referenced". This is the destructive cause in this bundle and sets the bundle's severity.
Fix: the DeleteFormType handler — add the asset-type reference lookup (over the stored asset-type → form-type reference index) ahead of the delete, and the ConflictException message assembly for that path. The related positive-control paths (PutAssetType referencing a missing form type → EntityNotFoundException; DeleteAssetType then DeleteFormType → success) are exercised by the same checks.
Traces: glue.delete-form-type-referenced-by-asset-type-fails, glue.form-type-delete-referenced-by-asset-type-rejected.

Cause 2 of 5: GetTrigger's unknown-trigger refusal message interpolates the requested name, which AWS does not.
AWS: Smithy: `GetTrigger` models `EntityNotFoundException` (errorKind client, httpError 400). The refusal body is HTTP 400 / `EntityNotFoundException` with Message the FIXED text `Trigger not found.` — there is no variable part and the requested `Name` is never echoed.
Emulator: HTTP 400 / `EntityNotFoundException` (status and code correct) with Message built from the request: `Trigger <Name> not found.` The replay recorded `'Trigger sim2real-behav-trigger-57940adbb65e not found.'`; the name there is that run's generated value, the defect is the template.
Why: a client (or a snapshot assertion) matching on the refusal text sees a sentence AWS never emits, and infers the service echoes the requested trigger name back in error bodies.
Fix: the GetTrigger handler's not-found refusal path (message construction) in the glue crate's trigger ops.
Note: open issue `20261029T201859Z-glue-not-found-refusals-carry-emulator-authored-messages.md` describes the same GetTrigger message divergence as its Cause 3, but it is keyed on wire fixture steps only and carries no behavioral replay key, so the check below is not covered by it. Close them together if that issue is fixed first; the check id here is the behavioral replay key.
Traces: glue.create-trigger-conditional-without-predicate-fails (the check's CreateTrigger assertions — InvalidInputException / `Predicate cannot be null or empty` — passed; it fails only on this GetTrigger message).

Cause 3 of 5: DeleteColumnStatisticsTaskSettings answers NotImplemented, so the not-found contract for the settings sub-resource is never honoured.
AWS: Smithy: `DeleteColumnStatisticsTaskSettings` (required DatabaseName, TableName; optional CatalogID) models `EntityNotFoundException` (client, 400), `InvalidInputException` and `OperationTimeoutException`. Deleting settings for a table that has none is refused with HTTP 400 / `EntityNotFoundException`, Message the fixed text `ColumnStatisticsTaskSettings entry for input DatabaseName and TableName does not exist` (no variable part — neither the database nor the table name is echoed). The same code and message are returned both for a table that genuinely exists but has no settings and for a database/table pair that does not exist at all; GetColumnStatisticsTaskSettings uses the identical code and text.
Emulator: every call returns error code `NotImplemented` with the message `operation DeleteColumnStatisticsTaskSettings is scaffolded but not implemented in this emulator`, so the refusal class a client branches on never appears. `glue.delete-column-statistics-task-settings-nonexistent-not-found` fails asserting `EntityNotFoundException`, got `'NotImplemented'`; `glue.column-statistics-task-settings-duplicate-create-rejected` fails on the same call, with `expected EntityNotFoundException, got NotImplemented`.
Why: a client cannot tell "no settings configured" from "service broken", and idempotent-cleanup logic that swallows EntityNotFoundException instead propagates a hard failure.
Fix: implement the DeleteColumnStatisticsTaskSettings handler in the glue crate (`services/service-com-amazonaws-glue-awsglue/src/ops/delete_column_statistics_task_settings.rs`), including this existence check and its exact EntityNotFoundException text.
Note: open issue `20261029T185330Z-glue-deletecolumnstatisticstasksettings-stub.md` records the bare scaffold stub; it does not state the error code or the message text this cause requires, and does not carry these behavioral replay keys.
Traces: glue.delete-column-statistics-task-settings-nonexistent-not-found, glue.column-statistics-task-settings-duplicate-create-rejected.

Cause 4 of 5: UpdatePartition answers NotImplemented, so a mutation against a deleted partition never produces the modeled not-found refusal.
AWS: Smithy: `UpdatePartition` (required DatabaseName, TableName, PartitionValueList, PartitionInput) models `EntityNotFoundException` (client, 400). Updating a partition that has been deleted is refused with HTTP 400 / `EntityNotFoundException` and Message following the template `Partition [<comma-separated PartitionValueList>] not found` — fixed text `Partition [`, `] not found` (no trailing period) with the partition values taken from the request. Note this text differs from GetPartition's, which for the same absent partition is the fixed `Cannot find partition.`; the GetPartition assertion in the same check passes, so the two messages must stay distinct.
Emulator: error code `NotImplemented` (`operation UpdatePartition is scaffolded but not implemented in this emulator`). `glue.partition-create-get-delete` fails with `a deleted partition must reject a mutation with EntityNotFoundException, got NotImplemented`.
Why: a client cannot perform a read-modify-write on partitions at all, and cannot distinguish a vanished partition from an emulator gap.
Fix: implement the UpdatePartition handler in the glue crate (`services/service-com-amazonaws-glue-awsglue/src/ops/update_partition.rs`) over stored partition state, with this existence check and message template.
Note: open issue `20261029T185330Z-glue-updatepartition-stub.md` records the bare scaffold stub without the error code or message text, and carries no behavioral replay key for this check.
Traces: glue.partition-create-get-delete.

Cause 5 of 5: a Data Quality ruleset evaluation run can never be started, so no DataQualityResult is ever produced or scored.
AWS: Smithy: `StartDataQualityRulesetEvaluationRun` (required DataSource, Role, RulesetNames) returns a `RunId`; `GetDataQualityRulesetEvaluationRun` reports a Status that reaches a terminal `SUCCEEDED` and a `ResultIds` list; `GetDataQualityResult` returns `RuleResults` (one entry per DQDL rule, each with a PASS/FAIL `Result`) and a `Score` (GenericBoundedDouble, range 0.0–1.0) equal to the fraction of rules that passed. For a 3-row CSV table and the ruleset `Rules = [ RowCount = 3, RowCount = 999, ColumnValues "status" in [ "ACTIVE", "INACTIVE" ] ]`, AWS evaluates the rules against the real object bytes: 3 RuleResults, 2 PASS, 1 FAIL, Score ≈ 2/3.
Emulator: StartDataQualityRulesetEvaluationRun never returns a RunId. The check's start helper retries every exception for its whole budget and the check is killed at the harness deadline (`Check timed out after 60.0s`), so the run id, the terminal status, ResultIds, RuleResults and Score are all unreachable. The underlying refusal is the scaffold stub for that operation.
Why: the entire data-quality feature is inert — a client gets no run to poll and no result to read, and a pipeline gated on a DQ score blocks forever rather than failing fast.
Fix: implement the StartDataQualityRulesetEvaluationRun handler in the glue crate (`services/service-com-amazonaws-glue-awsglue/src/ops/start_data_quality_ruleset_evaluation_run.rs`), the run's state transition to a terminal status, and the GetDataQualityResult projection (per-rule PASS/FAIL and the derived Score) so the result reflects the rules actually evaluated against the table's data.
Note: open issue `20261029T031345Z-glue-startdataqualityrulesetevaluationrun-stub.md` records the bare scaffold stub; it does not state the result/score semantics this cause requires, and carries no behavioral replay key for this check.
Traces: glue.e2e-data-quality-ruleset-evaluation-flow.

## Evidence
Behavioral suite replay (simparity-behavioral-aws-v1): 191/523 passed. Per-cause expected vs actual, quoted from the failing checks:

Cause 1 — DeleteFormType on a still-referenced form type.
  Expected: HTTP 400, code ConflictException, Message `{"Message":"Form type <formTypeName> cannot be deleted because of the following dependencies: gluetype glueAsset#glue_<accountId>#<assetTypeName>"}`; the form type survives (GetFormType still returns it).
  Actual (glue.delete-form-type-referenced-by-asset-type-fails): `AssertionError: expected ConflictException, got ('<succeeded>', '')` — the delete returned success.
  Actual (glue.form-type-delete-referenced-by-asset-type-rejected): `AssertionError: unexpected error code: None` after polling for the conflict — no attempt was ever refused.
  Positive controls in the same checks PASSED: PutAssetType naming a missing form type is refused EntityNotFoundException / `{"Message":"Metadata form type <name> not found."}`, and an unreferenced form type deletes cleanly.

Cause 2 — GetTrigger for a trigger that does not exist.
  Expected: HTTP 400, EntityNotFoundException, Message exactly `Trigger not found.`
  Actual (glue.create-trigger-conditional-without-predicate-fails): `AssertionError: unexpected not-found message 'Trigger sim2real-behav-trigger-57940adbb65e not found.'` — correct status and code, message template `Trigger <Name> not found.` (the id shown is that run's generated name, not part of the requirement).

Cause 3 — DeleteColumnStatisticsTaskSettings for a table with no settings.
  Expected: HTTP 400, EntityNotFoundException, Message `ColumnStatisticsTaskSettings entry for input DatabaseName and TableName does not exist`.
  Actual (glue.delete-column-statistics-task-settings-nonexistent-not-found): `AssertionError: expected EntityNotFoundException deleting settings that were never created, got 'NotImplemented'`.
  Actual (glue.column-statistics-task-settings-duplicate-create-rejected): `AssertionError: Delete on a table with no settings: expected EntityNotFoundException, got NotImplemented`.
  The emulator's message on that path is `operation DeleteColumnStatisticsTaskSettings is scaffolded but not implemented in this emulator`.

Cause 4 — UpdatePartition after DeletePartition.
  Expected: HTTP 400, EntityNotFoundException, Message `Partition [<values>] not found` (for PartitionValueList `["2023"]`: `Partition [2023] not found`).
  Actual (glue.partition-create-get-delete): `AssertionError: a deleted partition must reject a mutation with EntityNotFoundException, got NotImplemented`.
  The GetPartition assertion in the same check (EntityNotFoundException / `Cannot find partition.`) passed, so only the UpdatePartition path diverges.

Cause 5 — Data Quality ruleset evaluation end to end.
  Expected: StartDataQualityRulesetEvaluationRun returns a RunId; the run reaches Status SUCCEEDED; ResultIds has exactly 1 entry; GetDataQualityResult returns 3 RuleResults with 2 PASS / 1 FAIL and Score ≈ 0.667 for a 3-row table and the 3-rule DQDL ruleset.
  Actual (glue.e2e-data-quality-ruleset-evaluation-flow): `Error: Check timed out after 60.0s` — the start call never succeeded, so no RunId, no ResultIds, no RuleResults and no Score were ever observable.

All generated database/table/trigger/form-type names, account ids and run ids above are that run's values; the requirements are the status, the error code, and each message's fixed text plus the named request/state-derived variable parts.

## Steps to Reproduce
1. Clone this repo and replay the behavioral suite (run_behavioral_suite) against the booted emulator.
2. Re-replay exactly these checks, which are the ones this issue is grounded in:
   - glue.delete-form-type-referenced-by-asset-type-fails      (cause 1)
   - glue.form-type-delete-referenced-by-asset-type-rejected    (cause 1)
   - glue.create-trigger-conditional-without-predicate-fails    (cause 2)
   - glue.delete-column-statistics-task-settings-nonexistent-not-found (cause 3)
   - glue.column-statistics-task-settings-duplicate-create-rejected    (cause 3)
   - glue.partition-create-get-delete                           (cause 4)
   - glue.e2e-data-quality-ruleset-evaluation-flow              (cause 5)
3. Cause 1 by hand: glue.put_form_type(Name=F, Schema=...); glue.put_asset_type(Name=A, Forms={F: {FormTypeIdentifier: F}}); glue.put_asset(AssetTypeId=A, ...); then glue.delete_form_type(Identifier=F) — expect ConflictException with the dependency message and GetFormType(F) still returning the form type; the emulator returns success. Then delete_asset/delete_asset_type and repeat delete_form_type — it must now succeed and GetFormType must answer EntityNotFoundException.
4. Cause 2: glue.get_trigger(Name=<a name that was never created>) and compare $.Message to the fixed text `Trigger not found.`
5. Cause 3: create a database and table, then glue.delete_column_statistics_task_settings(DatabaseName=db, TableName=tbl) with no settings ever created — expect EntityNotFoundException / `ColumnStatisticsTaskSettings entry for input DatabaseName and TableName does not exist`; repeat with a database/table pair that does not exist and expect the identical code and message.
6. Cause 4: create a table with PartitionKeys=[{Name: year, Type: string}], create_partition Values=['2023'], delete_partition, then update_partition(PartitionValueList=['2023'], ...) — expect EntityNotFoundException / `Partition [2023] not found`.
7. Cause 5: create a CSV-backed external table with 3 rows (2 ACTIVE, 1 INACTIVE), create_data_quality_ruleset with `Rules = [ RowCount = 3, RowCount = 999, ColumnValues "status" in [ "ACTIVE", "INACTIVE" ] ]`, start_data_quality_ruleset_evaluation_run, poll get_data_quality_ruleset_evaluation_run to SUCCEEDED, then get_data_quality_result on the single ResultId and check 3 RuleResults / 2 PASS / 1 FAIL / Score ≈ 0.667.
