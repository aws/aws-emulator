Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bcm-data-exports.create-export-aggregate-query-rejected bcm-data-exports.create-export-resource-id-column-without-include-resources-rejected bcm-data-exports.create-export-unsupported-time-granularity-rejected bcm-data-exports.create-export-duplicate-name-rejected bcm-data-exports.update-export-changing-queried-table-rejected
Issue-Shape-Id: com.amazonaws.bcmdataexports#AWSBillingAndCostManagementDataExports
Issue-Key: bcm-data-exports.create-export-aggregate-query-rejected|_refusal#5|AssertionError|raise AssertionError(f"CreateExport accepted a definition it should refuse: {made!r}")
Issue-Key: bcm-data-exports.create-export-resource-id-column-without-include-resources-rejected|run#5|AssertionError|assert got == ("ValidationException", (
Issue-Key: bcm-data-exports.create-export-unsupported-time-granularity-rejected|run#6|AssertionError|assert got == ("ValidationException", (
Issue-Key: bcm-data-exports.create-export-duplicate-name-rejected|run#7|AssertionError|assert (code, message) == ("ValidationException", "Cannot create duplicate export name."), (
Issue-Key: bcm-data-exports.update-export-changing-queried-table-rejected|_update_refusal#9|AssertionError|raise AssertionError(f"UpdateExport accepted a query against another table: {done!r}")
Issue-Commit: d1c103039c63f2296b4493ce7c42775d3faa3fcc

# bcm-data-exports CreateExport/UpdateExport: aggregate query accepted, table-immutability unenforced, and three ValidationException messages are emulator-authored

> _Note: this issue was identified against the code base at commit `d1c103039c63f2296b4493ce7c42775d3faa3fcc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateExport

## Description
Five net-new root causes in this service's export-definition validation. All five are reached through the two operations that accept an `Export` definition (CreateExport, UpdateExport), which share `crate::state::query::resolve_data_query`.

Cause 1 of 5: CreateExport accepts a QueryStatement that applies an aggregate function to a CUR 2.0 column.
AWS: Smithy `CreateExport` documents "The QueryStatement is an SQL statement. Data Exports only supports a limited subset of the SQL syntax" (DataQuery.QueryStatement, @length(1,36000)); the Data Exports data-query subset is a plain column projection over a published table, so an aggregate/function projection is outside it. Recorded behavior: `SELECT SUM(<numeric column>) FROM COST_AND_USAGE_REPORT` is refused HTTP 400 `ValidationException`, Message exactly `Invalid QueryStatement` (fixed text, no request values interpolated), and no export is created. The same statement selecting the bare column (`SELECT <column> FROM COST_AND_USAGE_REPORT`) is accepted and stored verbatim.
Emulator: the call succeeds with HTTP 200 and returns an ExportArn; the export then shows up in ListExports under the submitted name. `resolve_data_query` only resolves the identifier references a statement makes (`SELECT SUM(c)` resolves `c`, a published column) and never judges the projection's SHAPE, so a function/aggregate projection passes.
Why: a client that relies on AWS rejecting an unsupported query gets a 200 and a persisted export that can never deliver; code paths that handle ValidationException to fall back to a legal projection are never exercised, and the account accumulates an export AWS would never have created.
Fix: the QueryStatement reader used by both definition-accepting handlers — the published-subset check in `crate::state::query` (`query_shape` / `resolve_data_query`), which must refuse a projection that is not a column reference or wildcard with the modelled ValidationException, before anything is persisted.
Traces: bcm-data-exports.create-export-aggregate-query-rejected.

Cause 2 of 5: the "column not published by the queried table" refusal carries the emulator's own sentence.
AWS: refusing a statement that names a column the queried table does not publish under the table configuration the request states (e.g. a resource-level column while INCLUDE_RESOURCES is FALSE) answers HTTP 400 `ValidationException` with Message `The columns in the query provided are not a subset of the table <TableName>: [<column>, ...]` — fixed text `The columns in the query provided are not a subset of the table `, then the table name from the request's statement, then `: [` and the offending column name(s) from the request, `]`. Model: ValidationException.Message is @required, @length(0,1024). The same query with the gating property set to TRUE is accepted.
Emulator: answers ValidationException with Message `Column name <column> is invalid` (observed: `Column name line_item_resource_id is invalid`), built in `invalid_query_column`.
Why: the message is part of the error contract — it is what names the TABLE the column was checked against and the full set of offending columns. A client (or a human reading a log) that matches or parses AWS's sentence sees different fixed text and loses the table name entirely.
Fix: the column refusal builder in `crate::state::query` (`invalid_query_column`), which must be given the resolved table name and the full offending column list.
Traces: bcm-data-exports.create-export-resource-id-column-without-include-resources-rejected.

Cause 3 of 5: the "table property value outside its valid values" refusal carries the emulator's own sentence.
AWS: a TableConfigurations entry whose property value is outside the property's published valid values is refused HTTP 400 `ValidationException` with Message `Invalid parameters: Value <value> does not match valid values [<v1>, <v2>, ...] for table <TableName> and table property <PropertyName>` — fixed text `Invalid parameters: Value `, ` does not match valid values [`, `] for table `, ` and table property `; the rejected value, the valid-value list, the table name and the property name all come from the request and from the published table dictionary. A legal value on the same definition is accepted and stored.
Emulator: answers `Value <value> is not supported for table property <PropertyName>. Valid values: [<v1>, <v2>, ...].` (observed for TIME_GRANULARITY=WEEKLY), built in `invalid_table_property_value` — different fixed text, different clause order, no table name, and a trailing period AWS does not emit.
Why: same as cause 2 — the sentence is the contract, and the table a property was judged for is missing from it.
Fix: the table-property value refusal builder in `crate::state::catalog` (`invalid_table_property_value`), which must receive the table name of the entry being judged.
Traces: bcm-data-exports.create-export-unsupported-time-granularity-rejected.

Cause 4 of 5: the duplicate-export-name refusal carries the emulator's own sentence.
AWS: a second CreateExport reusing a live export's name is refused HTTP 400 `ValidationException` with the FIXED Message `Cannot create duplicate export name.` — the whole sentence is fixed text; no name, ARN or other request value is interpolated. The live export is untouched, and the name becomes usable again after DeleteExport.
Emulator: refuses with the right code and status but Message `An export named <name> already exists; export names must be unique.`, interpolating the requested name (see `src/ops/create_export.rs`, the `store.export_named(...)` branch).
Why: a client matching AWS's fixed duplicate-name sentence to distinguish a name collision from any other ValidationException cannot do so.
Fix: the duplicate-name branch of the CreateExport handler.
Traces: bcm-data-exports.create-export-duplicate-name-rejected.

Cause 5 of 5: UpdateExport lets the replacement query change the TABLE the export reads.
AWS: UpdateExport "[updates] an existing data export by overwriting all export parameters", but the queried table is fixed at creation: a replacement definition whose statement selects FROM a different published table (e.g. moving a COST_AND_USAGE_REPORT export onto the FOCUS table) is refused HTTP 400 `ValidationException` with Message `'TableName' may not change after creation` (fixed text, the member name quoted as AWS quotes it), and the stored definition — query and TableConfigurations — is left exactly as it was. A rewrite of the statement over the SAME table is accepted, and an export created directly against the other table is legal.
Emulator: the update succeeds with HTTP 200 and the export's stored DataQuery (statement and TableConfigurations) is replaced with the other table's. `src/ops/update_export.rs` enforces immutability for `Export.Name` only; nothing compares the replacement statement's resolved tables with the stored ones.
Why: an export silently changes which cost dataset it delivers; a client relying on the refusal to keep a definition's table stable gets a mutated resource and no error.
Fix: the UpdateExport handler's pre-mutation validation, next to the existing Export.Name immutability check — compare the tables the replacement statement resolves to against the stored definition's and refuse with the modelled ValidationException before the store is written.
Traces: bcm-data-exports.update-export-changing-queried-table-rejected.

## Evidence
Cause 1 — expected: ClientError ValidationException / 400, Message 'Invalid QueryStatement', and ListExports showing no export under the submitted name. Actual: HTTP 200, {'ExportArn': 'arn:aws:bcm-data-exports:us-east-1:<account>:export/<name>/<uuid>'}; the check failed with "CreateExport accepted a definition it should refuse".

Cause 2 — expected: ('ValidationException', 'The columns in the query provided are not a subset of the table COST_AND_USAGE_REPORT: [line_item_resource_id]'). Actual: ('ValidationException', 'Column name line_item_resource_id is invalid').

Cause 3 — expected: ('ValidationException', 'Invalid parameters: Value WEEKLY does not match valid values [DAILY, MONTHLY, HOURLY] for table COST_AND_USAGE_REPORT and table property TIME_GRANULARITY'). Actual: ('ValidationException', 'Value WEEKLY is not supported for table property TIME_GRANULARITY. Valid values: [DAILY, MONTHLY, HOURLY].').

Cause 4 — expected: ('ValidationException', 'Cannot create duplicate export name.'). Actual: ('ValidationException', 'An export named sim2real-dupname-<id> already exists; export names must be unique.') — the <id> part is a per-run generated name and is not itself the defect; the defect is that AWS interpolates nothing at all here.

Cause 5 — expected: ClientError ValidationException, Message "'TableName' may not change after creation", with GetExport still reporting the original statement and only the original table in TableConfigurations. Actual: HTTP 200, {'ExportArn': 'arn:aws:bcm-data-exports:us-east-1:<account>:export/<name>/<uuid>'}; the check failed with "UpdateExport accepted a query against another table".

Source of the diverging messages: `invalid_query_column` (services/service-com-amazonaws-bcmdataexports-awsbillingandcostmanagementdataexports/src/state/query.rs:1004-1012), `invalid_table_property_value` (same crate, src/state/catalog.rs:548-563), the duplicate-name branch (src/ops/create_export.rs:289-301). Missing checks: projection shape in src/state/query.rs (`resolve_data_query`, lines 790-850) and table immutability in src/ops/update_export.rs (only Export.Name is compared, lines 188-205).

## Steps to Reproduce
Against the emulator with a delivery bucket whose policy grants bcm-data-exports.amazonaws.com s3:PutObject:
1. CreateExport with Export.DataQuery.QueryStatement = 'SELECT SUM(line_item_unblended_cost) FROM COST_AND_USAGE_REPORT' and TableConfigurations {'COST_AND_USAGE_REPORT': {'TIME_GRANULARITY': 'MONTHLY'}} -> observe HTTP 200 instead of 400 ValidationException 'Invalid QueryStatement' (cause 1).
2. CreateExport with QueryStatement selecting line_item_resource_id and TableConfigurations {'COST_AND_USAGE_REPORT': {'INCLUDE_RESOURCES': 'FALSE', 'TIME_GRANULARITY': 'DAILY'}} -> read Error.Message (cause 2).
3. CreateExport with TableConfigurations {'COST_AND_USAGE_REPORT': {'TIME_GRANULARITY': 'WEEKLY'}} -> read Error.Message (cause 3).
4. CreateExport a valid export, then CreateExport again with the same Export.Name -> read Error.Message (cause 4).
5. CreateExport against COST_AND_USAGE_REPORT, then UpdateExport the same ExportArn with a statement selecting FROM the FOCUS table and that table's TableConfigurations -> observe HTTP 200 and a GetExport whose DataQuery now names the FOCUS table (cause 5).
