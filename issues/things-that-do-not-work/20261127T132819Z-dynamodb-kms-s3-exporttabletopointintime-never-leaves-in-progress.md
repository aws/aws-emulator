Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__kms__s3.export-sse-algorithm-in-s3-header-spelling-rejected
Issue-Key: dynamodb__kms__s3.export-sse-algorithm-in-s3-header-spelling-rejected|_until_done#27|AssertionError|raise AssertionError(f"the export was still IN_PROGRESS after {budget_s:.0f}s")
Issue-Commit: 803b8f0256cf78557f71fb793e40c4a31d070d39

# dynamodb__kms__s3: ExportTableToPointInTime never leaves IN_PROGRESS, so the SSE-algorithm spelling refusal is never reached

> _Note: this issue was identified against the code base at commit `803b8f0256cf78557f71fb793e40c4a31d070d39`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExportTableToPointInTime

## Description
Cause 1 of 1: an export job started against a customer-managed-key table stays IN_PROGRESS indefinitely, and a request whose S3SseAlgorithm uses the S3 header spelling is not refused.
AWS: `ExportTableToPointInTime` validates `S3SseAlgorithm` against the DynamoDB enum (`AES256`, `KMS`) and refuses an S3-header spelling (e.g. `aws:kms`) with a ValidationException, HTTP 400, before the job starts. A valid export reaches `ExportStatus: COMPLETED` (or FAILED) and the manifest and data objects appear under the S3 prefix.
Emulator: `DescribeExport` reports `ExportStatus: IN_PROGRESS` for the whole wait window (`the export was still IN_PROGRESS after 0s`), so the job never terminates and the validation under test — which the check reaches only after a control export settles — never happens.
Why: a client polling DescribeExport waits forever on a job that will never finish, and a request AWS refuses up front is instead accepted and left pending.
Fix: the ExportTableToPointInTime handler and its export state machine — the job must progress to a terminal status, and S3SseAlgorithm must be validated against the DynamoDB enum before the job is created.
Traces: dynamodb__kms__s3.export-sse-algorithm-in-s3-header-spelling-rejected.

## Evidence
Expected: the control export reaches a terminal ExportStatus and the S3-header spelling of S3SseAlgorithm is refused with ValidationException (400). Actual: `the export was still IN_PROGRESS after 0s`.

## Steps to Reproduce
1. Create a table encrypted with a customer-managed KMS key and enable PITR.
2. Call ExportTableToPointInTime to an S3 bucket and poll DescribeExport — observe ExportStatus stays IN_PROGRESS.
3. Call ExportTableToPointInTime with S3SseAlgorithm set to the S3 header spelling — observe it is not refused.
