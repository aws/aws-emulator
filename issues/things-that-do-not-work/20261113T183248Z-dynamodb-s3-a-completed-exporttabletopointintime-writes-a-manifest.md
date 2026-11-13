Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__s3.export-to-s3-writes-the-exact-items-and-the-manifest-agrees
Issue-Key: dynamodb__s3.export-to-s3-writes-the-exact-items-and-the-manifest-agrees|run#30|AssertionError|assert summary['itemCount'] == _N_ITEMS, (
Issue-Commit: f2f8f36b8a9d5e45c8e23580863863f823670049

# dynamodb__s3: a COMPLETED ExportTableToPointInTime writes a manifest summary whose itemCount is 0 for a table that holds items

> _Note: this issue was identified against the code base at commit `f2f8f36b8a9d5e45c8e23580863863f823670049`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExportTableToPointInTime

## Description
1 net-new root cause in the DynamoDB table-export manifest. Related open issues carry neither this replay key nor this service: 20261113T163227Z-cloudformation-dynamodb-s3-exporttabletopointintime-reports-completed-but-its-manifest-summary-says.md and 20261108T043811Z-cloudformation-dynamodb-s3-dynamodb-exporttabletopointintime-reports-completed-without-authorizing-the-s3.md (both filed under cloudformation__dynamodb__s3).

Cause 1 of 1: the export's manifest-summary object reports itemCount 0 regardless of how many items the table holds.
AWS: a full export that reaches ExportStatus COMPLETED writes the summary object named by ExportDescription.ExportManifest; that JSON's itemCount is the number of items exported, and manifestFilesS3Key names a newline-delimited file whose per-data-file itemCount values partition that same total, with the data files gzipped DynamoDB JSON. Smithy: ExportDescription.ItemCount — "The number of items exported" (long, range min 0) — and ExportDescription.ExportManifest — "The name of the manifest file for the export task"; the summary object's own field names are recorded AWS behavior.
Emulator: the export reaches COMPLETED and the summary object exists, but summary['itemCount'] is 0 while the table held 12 items written before the export was started.
Why: a consumer that sizes or validates an export from its manifest (the documented way: compare itemCount with the table's item count, then fan out over manifestFilesS3Key) concludes the export is empty and either skips the data files or reports data loss; a pipeline gated on itemCount > 0 never runs.
Fix: the export writer in the dynamodb service — the manifest-summary serializer must count the items actually written to the data files (and keep the per-data-file itemCount values summing to it).
Traces: dynamodb__s3.export-to-s3-writes-the-exact-items-and-the-manifest-agrees.

## Evidence
Cause 1: AssertionError: the manifest itemCount disagrees with the table: 0, expected 12 — read from the object named by DescribeExport's ExportDescription.ExportManifest after ExportStatus became COMPLETED for a table with 12 PutItem-written items.

## Steps to Reproduce
Cause 1: create a PAY_PER_REQUEST table, enable point-in-time recovery, PutItem 12 items, call ExportTableToPointInTime(TableArn, S3Bucket, S3Prefix='exp', ExportFormat='DYNAMODB_JSON'), poll DescribeExport to COMPLETED, GetObject the key in ExportDescription.ExportManifest and read its itemCount.
