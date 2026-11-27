Issue-Source: trace-sweep-divergence
Issue-Trace: athena-aorigin-athena-metadata-table-vs-view
Issue-Step: task#7
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: athena-aorigin-athena-metadata-table-vs-view athena-cflog-athena-catalog-lookup-mistakes athena-glue-metadata-exploration
Issue-Shape-Id: com.amazonaws.athena#AmazonAthena
Issue-Key: athena|GetTableMetadata|4xx->4xx|$.Message
Issue-Key: athena|ListTableMetadata|4xx->4xx|$.Message
Issue-Commit: 2eb3e162f2b8ae792971ae16f8b17dcde772ffd5

# athena metadata-lookup refusals omit the wrapped AmazonDataCatalog EntityNotFoundException envelope (2 causes)

> _Note: this issue was identified against the code base at commit `2eb3e162f2b8ae792971ae16f8b17dcde772ffd5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTableMetadata

## Description
2 causes. Both are the same fault in the athena crate's catalog-metadata read path: the MetadataException raised when a requested catalog entity does not exist carries emulator-authored bare text instead of the relayed AmazonDataCatalog exception wrapped in AWS's envelope. The HTTP status (400) and the error code (MetadataException) are correct in both causes; only the Message diverges. Both fixture steps assert Message as a SHAPE rather than a literal (the replay reports expected '<shaped-not-asserted>') precisely because AWS's message for this error class ends in a per-request Request ID, which is a per-run value and therefore not itself a requirement; what IS the requirement is the rest of the message, which is fixed text plus values taken from the request.

Cause 1 of 2: GetTableMetadata for a table that does not exist in an existing database.
AWS: Smithy (services/service-com-amazonaws-athena-amazonathena) GetTableMetadata takes required CatalogName (@length 1-256, @pattern ^[\u0020-\uD7FF\uE000-\uFFFD\uD800\uDC00-\uDBFF\uDFFF\t]*$), required DatabaseName and TableName (@length 1-128) and optional WorkGroup, and models errors InternalServerException (server, HTTP 500), InvalidRequestException (client, HTTP 400) and MetadataException (client, HTTP 400). The model does not state message text, so for the text this is recorded behavior only: against the AwsDataCatalog catalog Athena relays the Glue/AmazonDataCatalog failure and answers HTTP 400 / MetadataException with a message of the fixed form 'Entity Not Found (Service: AmazonDataCatalog; Status Code: 400; Error Code: EntityNotFoundException; Request ID: <request id>; Proxy: null)'. Everything in that message is fixed text for this error class except <request id>, which is per-run; 'Entity Not Found' is the relayed catalog message and the parenthesised part names the relaying service, the relayed status, the relayed error code EntityNotFoundException and the proxy.
Emulator: answers HTTP 400 with code MetadataException but Message 'Entity Not Found' — the relayed text alone, with the entire '(Service: AmazonDataCatalog; Status Code: 400; Error Code: EntityNotFoundException; Request ID: ...; Proxy: null)' envelope absent, so the recorded message shape does not match.
Why: a client that parses the envelope to recover the underlying catalog error code EntityNotFoundException — or that matches AWS's full message — cannot distinguish an absent-entity condition from any other metadata failure, so create-if-missing and retry/fallback logic keyed on EntityNotFoundException never fires.
Fix: the athena crate's GetTableMetadata handler, in the entity-not-found branch: render the relayed AmazonDataCatalog exception as the wrapped envelope message rather than emitting the relayed text alone.
Traces: athena-aorigin-athena-metadata-table-vs-view#task#7; also athena-cflog-athena-catalog-lookup-mistakes#task#4 and athena-glue-metadata-exploration#task#7 in the same cluster.

Cause 2 of 2: ListTableMetadata for a database that does not exist in the named catalog.
AWS: Smithy (same model) ListTableMetadata takes required CatalogName and DatabaseName plus optional Expression (@length 0-256), NextToken (@length 1-1024), MaxResults (@range 1-50) and WorkGroup, is @paginated, and models the same three errors including MetadataException (client, HTTP 400). The model does not state message text, so recorded behavior only: the recorded refusal is HTTP 400 / MetadataException whose Message is likewise the relayed AmazonDataCatalog exception carrying the '(Service: AmazonDataCatalog; Status Code: 400; Error Code: EntityNotFoundException; Request ID: <request id>; Proxy: null)' envelope — the fixture asserts it as a shape because of that per-run request id. The fixed text of the envelope is the requirement; the database name is the only part taken from the request. Hypothesis: the relayed leading clause for an absent database is Glue's own entity-not-found text rather than an Athena-authored sentence; the recording pins only the shape, so the envelope is what is asserted.
Emulator: answers HTTP 400 with code MetadataException but Message 'Database <databaseName> not found.' — an emulator-authored sentence (the database name is the request's DatabaseName; the rest is the emulator's own fixed text), with no relayed-exception envelope, so the recorded message shape does not match. Note the same emulator also produces a third, different wording for the same condition elsewhere ('Database does not exist: <databaseName>' in the DDL path), which shows the text is authored per handler rather than relayed.
Why: the absent-database condition is indistinguishable from any other metadata failure for a client that keys on the relayed EntityNotFoundException code, and a client string-matching AWS's message never matches, so database-bootstrap-on-demand flows take the wrong branch.
Fix: the athena crate's ListTableMetadata handler, in the database-not-found branch: relay the catalog exception with the wrapped envelope, from the same shared renderer Cause 1 should use, instead of authoring per-handler text.
Traces: athena-cflog-athena-catalog-lookup-mistakes#task#5.

Related OPEN issues that do NOT cover these causes: 20261114T133324Z-athena-cloudformation-glue-gettablemetadata-s-metadataexception-for-an.md and 20261104T082656Z-athena-glue-gettablemetadata-s-metadataexception-for-a-dropped.md are behavioral-suite issues that carry behavioral check keys only (athena__cloudformation__glue.glue-get-table-for-absent-entity-rejected and athena__glue.athena-table-metadata-follows-a-glue-table-drop-and-recreate). Neither carries the replay key for these wire fixture steps — the replay reports both clusters here as unowned — and neither mentions ListTableMetadata at all. A fixer touching the shared renderer should re-replay both fixture steps named above.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, 394 steps, 307 passed, score 77.9%).

Cause 1 — fixture athena-aorigin-athena-metadata-table-vs-view, step task#7, operation GetTableMetadata, verdict mismatch:
  expected_status 400, actual_status 400
  $.Message: value_mismatch (expected '<shaped-not-asserted>', got 'Entity Not Found')
  cluster key: service=athena|GetTableMetadata|status=400->400|keys=$.Message
  affected (3): athena-aorigin-athena-metadata-table-vs-view#task#7, athena-cflog-athena-catalog-lookup-mistakes#task#4, athena-glue-metadata-exploration#task#7
  Required message for this error class: 'Entity Not Found (Service: AmazonDataCatalog; Status Code: 400; Error Code: EntityNotFoundException; Request ID: <request id>; Proxy: null)' — all fixed text except the per-run <request id>. Actual: 'Entity Not Found'.

Cause 2 — fixture athena-cflog-athena-catalog-lookup-mistakes, step task#5, operation ListTableMetadata, verdict mismatch:
  expected_status 400, actual_status 400
  $.Message: value_mismatch (expected '<shaped-not-asserted>', got 'Database sim2real_probe_cflog_none_f3f8c76f not found.')
  cluster key: service=athena|ListTableMetadata|status=400->400|keys=$.Message
  affected (1): athena-cflog-athena-catalog-lookup-mistakes#task#5
  In the actual message the database name is the request's DatabaseName (the recorded run's value 'sim2real_probe_cflog_none_f3f8c76f' is a per-run probe name, not part of the requirement) and 'Database ... not found.' is the emulator's own fixed text; the relayed '(Service: AmazonDataCatalog; Status Code: 400; Error Code: EntityNotFoundException; Request ID: <request id>; Proxy: null)' envelope is absent.

Status and error code match in both clusters; only $.Message diverges.

## Steps to Reproduce
1. git_clone the repo and let the wire corpus build and boot the emulator.
2. Replay trace set simparity-wire-aws-v1 and inspect fixture athena-aorigin-athena-metadata-table-vs-view step task#7 and fixture athena-cflog-athena-catalog-lookup-mistakes step task#5.
3. Manual repro for Cause 1: create a Glue database and a table in it, confirm athena.get_table_metadata(CatalogName='AwsDataCatalog', DatabaseName=<db>, TableName=<tbl>) resolves. Then call athena.get_table_metadata(CatalogName='AwsDataCatalog', DatabaseName=<db>, TableName=<absent table>). Expect HTTP 400 / MetadataException with Message 'Entity Not Found (Service: AmazonDataCatalog; Status Code: 400; Error Code: EntityNotFoundException; Request ID: <request id>; Proxy: null)'; observe Message 'Entity Not Found'.
4. Manual repro for Cause 2: call athena.list_table_metadata(CatalogName='AwsDataCatalog', DatabaseName=<absent database>). Expect HTTP 400 / MetadataException whose Message is the relayed AmazonDataCatalog exception carrying the '(Service: AmazonDataCatalog; Status Code: 400; Error Code: EntityNotFoundException; Request ID: <request id>; Proxy: null)' envelope; observe the emulator-authored 'Database <databaseName> not found.' with no envelope.
5. Re-replay both fixture steps after changing the handler to confirm both causes are closed.
