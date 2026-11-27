Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__s3.export-format-encodes-every-attribute-type-as-json-or-ion dynamodb__s3.filtered-export-key-attribute-in-filter-beside-key-condition-rejected dynamodb__s3.export-with-sse-aes256-writes-encrypted-dynamodb-json-items dynamodb__s3.export-with-s3-style-sse-algorithm-name-rejected dynamodb__s3.import-target-table-is-creating-and-unwritable-until-import-ends
Issue-Key: dynamodb|ExportTableToPointInTime|NotImplemented
Issue-Key: dynamodb__s3.filtered-export-key-attribute-in-filter-beside-key-condition-rejected|run#19|AssertionError|assert refused == (
Issue-Key: dynamodb__s3.export-with-sse-aes256-writes-encrypted-dynamodb-json-items|_settled_export#17|AssertionError|raise AssertionError(f"the export was still IN_PROGRESS after {wait_s:.0f}s")
Issue-Key: dynamodb__s3.export-with-s3-style-sse-algorithm-name-rejected|_final_status#26|AssertionError|raise AssertionError(f"export still IN_PROGRESS after {wait_s:.0f}s")
Issue-Key: dynamodb__s3.import-target-table-is-creating-and-unwritable-until-import-ends|run#11|AssertionError|assert status == "CREATING", f"the import's target table is {status!r} while the import runs"
Issue-Commit: 803b8f0256cf78557f71fb793e40c4a31d070d39

# dynamodb__s3: ExportTableToPointInTime is partly a scaffold (ION format, FilterSpecification), exports never leave IN_PROGRESS, and an import's target table reports ACTIVE while the import runs (4 causes)

> _Note: this issue was identified against the code base at commit `803b8f0256cf78557f71fb793e40c4a31d070d39`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExportTableToPointInTime

## Description
Four net-new root causes in the DynamoDB export/import to S3 path.

Cause 1 of 4: `ExportFormat: ION` is an unimplemented scaffold.
AWS: ExportTableToPointInTime accepts `ExportFormat` DYNAMODB_JSON or ION; with ION the exported objects encode every attribute type as Amazon Ion and the job completes normally.
Emulator: the call is refused with code `NotImplemented` and the message `operation ExportTableToPointInTime with ExportFormat ION is scaffolded but not implemented in this emulator` — a sentence AWS never emits and a code the operation does not model.
Why: half of the operation's modeled export formats is unusable, and the refusal is not a modeled DynamoDB error so no client can classify or retry it.
Traces: dynamodb__s3.export-format-encodes-every-attribute-type-as-json-or-ion.

Cause 2 of 4: `FilterSpecification` is an unimplemented scaffold.
AWS: a filtered export validates the filter — a key attribute used in the filter beside a key condition is refused with a ValidationException, HTTP 400 — and otherwise exports only the matching items.
Emulator: the call is refused with code `NotImplemented` and `operation ExportTableToPointInTime with FilterSpecification is scaffolded but not implemented in this emulator`, so the validation AWS performs never happens and no filtered export can be taken.
Why: the operation silently loses a modeled capability, and a request that AWS refuses as invalid is refused for an unrelated reason with an unmodeled code.
Traces: dynamodb__s3.filtered-export-key-attribute-in-filter-beside-key-condition-rejected.

Cause 3 of 4: an accepted export never reaches a terminal status.
AWS: `DescribeExport` on a started export moves to `COMPLETED` (or FAILED) and the manifest and data objects appear under the S3 prefix; with `S3SseAlgorithm: AES256` the written objects are SSE-S3 encrypted DynamoDB-JSON items.
Emulator: `ExportStatus` stays `IN_PROGRESS` for the whole wait window — `the export was still IN_PROGRESS after 1s` — and no objects are written.
Why: a client polling DescribeExport waits on a job that never finishes; the export data it is waiting for never appears.
Traces: dynamodb__s3.export-with-sse-aes256-writes-encrypted-dynamodb-json-items, dynamodb__s3.export-with-s3-style-sse-algorithm-name-rejected.

Cause 4 of 4: an import's target table reports ACTIVE and is writable while the import is running.
AWS: the table ImportTable creates is `CREATING` for the duration of the import and rejects data-plane writes until the import reaches a terminal status; only then does it become ACTIVE.
Emulator: `DescribeTable` reports `ACTIVE` while the import is still running, so the table accepts writes that can race the import's own loading.
Why: a client that waits on table status proceeds too early and can write items the import then overwrites, or read a half-loaded table believing it complete.
Fix for all four: the ExportTableToPointInTime handler and its export state machine (causes 1–3: implement the ION format and FilterSpecification, and drive the job to a terminal status) and the ImportTable handler's target-table status machine (cause 4).
Traces: dynamodb__s3.import-target-table-is-creating-and-unwritable-until-import-ends.

## Evidence
Cause 1: `botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ExportTableToPointInTime operation: operation ExportTableToPointInTime with ExportFormat ION is scaffolded but not implemented in this emulator`.
Cause 2: refused with `('NotImplemented', 'operation ExportTableToPointInTime with FilterSpecification is scaffolded but not implemented in this emulator')`, expected a ValidationException (400) for a key attribute in the filter beside a key condition.
Cause 3: `the export was still IN_PROGRESS after 1s` / `export still IN_PROGRESS after 1s`.
Cause 4: `the import's target table is 'ACTIVE' while the import runs`.

## Steps to Reproduce
1. Enable PITR on a table and call ExportTableToPointInTime with ExportFormat ION.
2. Call ExportTableToPointInTime with a FilterSpecification naming a key attribute beside a key condition.
3. Call ExportTableToPointInTime with S3SseAlgorithm AES256 and poll DescribeExport.
4. Call ImportTable to create a new table from an S3 prefix and call DescribeTable while the import is running.
