Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__dynamodb__s3.dynamodb-export-table-to-s3-positive
Issue-Key: cloudformation__dynamodb__s3.dynamodb-export-table-to-s3-positive|run#16|AssertionError|assert summary["itemCount"] == 3, (
Issue-Commit: b5b438418db974c5623067148485e4df5aeb5fcd

# ExportTableToPointInTime reports COMPLETED but its manifest summary says itemCount 0 — no table items are exported

> _Note: this issue was identified against the code base at commit `b5b438418db974c5623067148485e4df5aeb5fcd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExportTableToPointInTime

## Description
One net-new cause.

Cause 1 of 1: a DynamoDB table export to S3 reaches ExportStatus COMPLETED, but the manifest it writes reports an empty export for a non-empty table. Failing check: cloudformation__dynamodb__s3.dynamodb-export-table-to-s3-positive.

AWS: ExportTableToPointInTime(TableArn, S3Bucket, ExportFormat=DYNAMODB_JSON) on a PITR-enabled table completes and writes a full object set into the destination bucket. The summary object named by ExportDescription.ExportManifest carries 'itemCount' equal to the number of items exported — the table's item count at the export point in time — and 'manifestFilesS3Key' pointing at a manifest-files object with one JSON line per data file, each giving 'dataFileS3Key' and that file's 'itemCount'; those per-file counts sum to the exported item count, and each gzip'd DYNAMODB_JSON data file decodes to one {"Item": {...}} line per exported item, so the decoded set equals exactly what the table held. Recorded behavior only for the manifest's contents (the Smithy model describes ExportDescription/ExportManifest but not the document's fields).

Emulator: the export does reach COMPLETED and a summary object does exist at the ExportManifest key, but its 'itemCount' is 0 while the table holds 3 items — so the export carries none of the table's data (the check never gets as far as the manifest-files object or the data files).

Why: an export that silently contains nothing is reported as a success. A client that drives an export and then loads or verifies the data from the manifest — the documented consumption path for backups, cross-account copies and analytics loads — reads zero items and has no signal in ExportStatus, FailureCode or FailureMessage that anything is wrong; a restore or downstream load built on it loses the table's contents.

Fix: the ExportTableToPointInTime worker in the dynamodb crate: it must enumerate the table's items at the export point in time, write them as gzip'd DYNAMODB_JSON data files into the destination bucket, and emit a summary object and a manifest-files object whose itemCounts agree with the items actually written.

Traces: cloudformation__dynamodb__s3.dynamodb-export-table-to-s3-positive. Open issue 20261108T043811Z-cloudformation-dynamodb-s3-dynamodb-exporttabletopointintime-reports-completed-without-authorizing-the-s3.md covers a DIFFERENT cause on the same operation (the export is reported COMPLETED without authorizing the S3 write) and does not carry this check's replay key or the manifest's contents.

## Evidence
Behavioral replay of simparity-behavioral-aws-v1 against the emulator built from this checkout (810/2121 checks passed; the emulator is up and answering).

cloudformation__dynamodb__s3.dynamodb-export-table-to-s3-positive [mismatch]:
  assert summary["itemCount"] == 3, (f"manifest itemCount disagrees with the table: {summary['itemCount']!r}, expected 3")
  AssertionError: manifest itemCount disagrees with the table: 0, expected 3

The check creates a PAY_PER_REQUEST table keyed on 'pk', waits for it to exist, enables PITR and waits for PointInTimeRecoveryStatus ENABLED, PutItem's three items ({"pk":{"S":"item-<i>"},"val":{"N":"<i>"}} for i in 0..2), creates a destination bucket and calls export_table_to_point_in_time(TableArn, S3Bucket, ExportFormat='DYNAMODB_JSON'). Those legs pass: the export reaches ExportStatus COMPLETED (asserted) and GetObject on final['ExportManifest'] returns a parseable summary document. The 3 is the number of items written by this run; the requirement is that the manifest's itemCount equals the table's item count, that the per-data-file itemCounts partition it, and that the decoded data files equal the items written (the two later assertions this failure blocks).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay cloudformation__dynamodb__s3.dynamodb-export-table-to-s3-positive.
2. By hand: CreateTable (BillingMode PAY_PER_REQUEST, HASH key 'pk' of type S) and wait for ACTIVE; UpdateContinuousBackups with PointInTimeRecoveryEnabled=true and wait for PointInTimeRecoveryStatus ENABLED; PutItem three distinct items.
3. CreateBucket a destination bucket, then ExportTableToPointInTime(TableArn=<table arn>, S3Bucket=<bucket>, ExportFormat='DYNAMODB_JSON'); poll DescribeExport until a terminal status — expect COMPLETED (it is today).
4. GetObject the key in ExportDescription.ExportManifest and parse it: 'itemCount' must be 3. The emulator reports 0.
5. Then GetObject the key in 'manifestFilesS3Key': each line must give a 'dataFileS3Key' and an 'itemCount' summing to 3, and gunzipping each data file must yield one {"Item": ...} line per item, equal to the three items written.
