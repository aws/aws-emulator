Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__logs__s3.import-duplicate-keys-collapse-to-one-item-without-error-or-log dynamodb__logs__s3.import-item-errors-are-logged-to-the-imports-log-group dynamodb__logs__s3.import-with-undefined-sort-key-attribute-rejected
Issue-Key: dynamodb__logs__s3.import-duplicate-keys-collapse-to-one-item-without-error-or-log|run#29|AssertionError|assert time.monotonic() < log_deadline, (
Issue-Key: dynamodb__logs__s3.import-item-errors-are-logged-to-the-imports-log-group|run#21|AssertionError|assert streams, f"no log stream for import {import_id} in {_LOG_GROUP} after the import ended"
Issue-Key: dynamodb__logs__s3.import-with-undefined-sort-key-attribute-rejected|run#37|AssertionError|assert time.monotonic() < log_deadline, (
Issue-Commit: 803b8f0256cf78557f71fb793e40c4a31d070d39

# dynamodb__logs__s3: ImportTable publishes nothing to the /aws-dynamodb/imports CloudWatch log group

> _Note: this issue was identified against the code base at commit `803b8f0256cf78557f71fb793e40c4a31d070d39`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ImportTable

## Description
Cause 1 of 1: an import job writes no log stream and no events to its CloudWatch Logs destination.
AWS: `ImportTable` logs to the log group named in the request (conventionally `/aws-dynamodb/imports`), creating a log stream for the import whose name carries the import id and writing at least a start event when the import begins, per-item error events for rows it could not apply, and a completion event. Those events are the only record of which rows an import dropped.
Emulator: after the import has started — and after it has ended — the log group holds no stream for the import and `FilterLogEvents` returns `[]`. The import itself proceeds, so the absence is in the logging leg, not the import.
Why: an operator has no way to see that an import ran, or which items it rejected; duplicate keys that collapse into one item and rows refused for an undefined sort-key attribute are silently lost with no diagnostic anywhere.
Fix: the ImportTable handler's CloudWatch Logs publication — create the per-import log stream and emit the start, per-item error and completion events.
Traces: dynamodb__logs__s3.import-duplicate-keys-collapse-to-one-item-without-error-or-log, dynamodb__logs__s3.import-item-errors-are-logged-to-the-imports-log-group, dynamodb__logs__s3.import-with-undefined-sort-key-attribute-rejected.

## Evidence
`the import's start event never reached /aws-dynamodb/imports; its events are []`; `no log stream for import 01795784555875-c3538f1b in /aws-dynamodb/imports after the import ended`; `the accepted import's start event never reached /aws-dynamodb/imports: []`.

## Steps to Reproduce
1. Create the /aws-dynamodb/imports log group.
2. Upload DynamoDB-JSON items (including a duplicate key and a row missing the sort-key attribute) to an S3 prefix.
3. Call ImportTable naming that log group, wait for the import to reach a terminal status, then DescribeLogStreams / FilterLogEvents on the group.
